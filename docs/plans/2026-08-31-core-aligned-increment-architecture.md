# core.dll / Core3D.dll 增量更新机制取证 → e3d-model 增量架构改造设计

- 日期：2026-08-31
- 会话：`BajieAsk-agent-1-800a23b3`（承接 `b8ce34bd` 第一次活桥、`b9fc69a5` 审核）
- 取证方式：ida-bridge 活桥，**core.dll 带完整 MSVC 符号**，逐函数反编译 + 关键处读指令流
- 状态：**部分实施**（2026-08-31 21:3x 由会话 `f40baab7` 按磁盘实测回填）。
  §3 列的六项改造完成两项：**L2 `element_diff.rs` 与 L3 `ledger.rs` 已实装并随
  `e3d-model` 提交 `491d0e3` 入库**，真库门五窗、118 项测试、clippy、fmt 全绿（本机复验）。
  未做：§3.3 P0-2 隐式管身、§3.5「已产出模型集」输入、§3.6 几何输入摘要。
  **§3.1 的 L4「换 `noun.toplevel`」原方案已被自己的实测推翻**（两个集合互不包含），
  新方案「加一层更粗的键」只有一句话、尚无设计——**这条线目前就停在 L4 这个位置。**
  逐条状态见 §4 表的「今日实测」列。

## 0. 可复查地址表

| 层 | 函数 | 地址 | 实例 |
|---|---|---|---|
| L1 | `DB_IndexTableCompare::DB_IndexTableCompare(DB_DB*,int,int)` | `0x5a18b20` | core.dll |
| L1 | `DB_SystemTableCompare::{dbele,inserted,modified,deleted,next,finished}` | `0x5a18d10 / d90 / da0 / d70 / db0 / d80` | core.dll |
| L1 | `DB_DB::elementsChangedBetween(int,int,int,int,vector<DB_Element>&)` | `0x58ffb20` | core.dll |
| L1 | `DB_RawChanges::iterate(DB_IndexTableCompare&)` | `0x5983c30` | core.dll |
| L2 | `DB_Compare::scan` / `scanOld` / `scanNew` | `0x5a46600 / 0x5a46a40 / 0x5a46730` | core.dll |
| L2 | **`DB_Compare::checkEle`** | `0x5a45cd0` | core.dll |
| L2 | `DB_Compare::dealWithChangeType` | `0x5a46460` | core.dll |
| L2 | `DB_Element::hasElementChangedBetween` | `0x593d6d0` | core.dll |
| L2 | `DB_Element::attributesChangedBetween` | `0x5928100` | core.dll |
| L2 | `DB_Element::hasAttributeChangedBetween` | `0x593b930` | core.dll |
| L2 | `DB_Element::dabAttributesChangedBetween` | `0x592ba80` | core.dll |
| L3 | `DB_DB::elementsChangedBetween(…, DB_UserChanges&)` | `0x58ffc50` | core.dll |
| L3 | `DB_RecordCompare::{attChange,createAfter,deleteAll,includeAfter}` | `0x59a07f0 / 0910 / 0a90 / 0b50` | core.dll |
| L3 | `DB_UserChanges::elementIncluded` | `0x5987ea0` | core.dll |
| L3 | `DB_RawChanges::isElementTopLevelCreate` / `…Delete` | `0x5983b80 / 0x5983bd0` | core.dll |
| L4 | `DES_DrawListManager::changesBetweenSessions` | `0x1052b730` | Core3D.dll |
| L4 | `DES_DrawListManager::PostLocalChanges` | `0x1052aea0` | Core3D.dll |
| L4 | `DES_DrawListManager::{processCreates,processDeletes,processModifications}` | `0x1052cc10 / cd20 / cf80` | Core3D.dll |
| L4 | `DES_DrawListManager::findTopLevelElement` | `0x1052c450` | Core3D.dll |
| L4 | `desdblib/FNDTOP` | `0x10380E38` | Core3D.dll |
| L4 | `nounlib/LISTOP` | `0x561c93c` | core.dll |
| L4 | `DES_DrawList::getAncestorTOPFElement` / `getChildTOPFElements` | `0x10515fc0 / 0x105166d0` | Core3D.dll |
| L4 | `DES_DrawListManager::hasTopLevelGraphicsChanged` | `0x1052c850` | Core3D.dll |
| — | `DB_Noun::toplevel` | `0x58dba70` | core.dll |

---

## 1. core 的增量更新是**五层管线**，不是一趟循环

```
L0  会话定位     switchToOldSession(db, sesno, extno) —— 把整个库读窗钉到某个历史会话
L1  索引候选     DB_IndexTableCompare  →  (元素, inserted|modified|deleted)     【只是候选】
L2  逐元素分类   DB_Compare::checkEle  →  12 种回调                             【权威判据】
L3  语义账       DB_UserChanges        →  8 个语义桶
L4  消费者上卷   findTopLevelElement / getAncestorTOPFElement → 顶层可画单元
```

**最关键的一条：L1 在 core 里只是候选过滤器，不是结论。** e3d-model 现在把 L1 的
`Modified` 直接当成「这个元素变了」，然后跳过 L2/L3，用几何启发式（比 POS/ORI）补判。
core 从不这么做——它拿到候选后一定回去做**记录级、逐属性、按类型比值**的权威差分。

### L1 索引候选（我们已有，且已对齐）

`DB_DB::elementsChangedBetween`（`0x58ffb20`）的形状与 e3d-io 的 `IndexDiff` 同构：

```
switchToOldSession(db, target_sesno, target_extno)      # 先把库钉到目标端
DB_IndexTableCompare cmp(db, base_sesno, base_extno)    # 再对 base 端做索引比较
while !cmp.finished():
    if cmp.modified(): out.push(cmp.dbele())
    cmp.next()
switchBackSession()
```

`DB_RawChanges::iterate`（`0x5983c30`）把同一个迭代器分成三桶：`modified()` / `inserted()` / 其余（deleted）。
→ **A2「删除侧钉 base 端点」已对齐；`Modified = RecordPosition 变` 这条地基是 core 自己的判据，
我们继承它不算独有风险。**

### L2 逐元素分类 —— `DB_Compare::checkEle`，我们完全没有这一层

`0x5a45cd0` 反编译后的算法（vtable 槽位按 `DB_RecordCompare` 的实现回填）：

```
checkEle(el):
    switchToOldSession(BASE)
    if !el.isValid():                       # base 端不存在 → 新建
        switchToOldSession(TARGET)
        if el.dabPrev() OK:   createAfter (el, prev)      # 带插入位置
        elif el.dabOwner() OK: createFirst(el, owner)
        else:                  createNull (el)
        return

    switchToOldSession(TARGET)
    if <成员表里的位置变了>:
        if el.dabPrev() OK: reorderAfter(el, prev)  else: reorderFirst(el, owner)

    if hasAttributeChangedBetween(el, ATT_OWNER):         # ★ 改挂 = OWNER 这个属性变了
        if el.dabPrev() OK: includeAfter(el, prev)  else: includeFirst(el, owner)

    if stopAtFirstAttributeChange:
        for id in dabAttributesChangedBetween(el): attChange (el, id)
        for id in dabRulesChangedBetween(el):      ruleChange(el, id)
    else:
        if dabAnyAttributesChangedBetween(el):     attChange(el, 0)      # 0 = 「变了，不说哪个」

    if !noun.primaryList():
        for m in <次层级成员差分>:
            kind==2 → insertInSecondaryList(m, el)
            kind==1 → removeFromSecondaryList(m, el)
```

删除侧由 `scanOld` 反向扫出（base 有、target 无）→ `deleteAll(el)`。
元素**类型**变了走 `dealWithChangeType`（`0x5a46460`）：先 `deleteAll`，再把整棵子树按
`createAfter/createFirst/createNull` 重建。

**这 12 个回调就是 core 对「一次改动可能是什么」的完整定义**，也就是
`docs/specs/…conformance.md` E3 / ADR-009 要的那个「用户语义分类器」的权威原型：

| 回调 | 语义 |
|---|---|
| `createFirst` / `createAfter` / `createNull` | 新建（带在属主成员表中的插入位置） |
| `deleteAll` | 删除 |
| `includeFirst` / `includeAfter` | **改挂**（OWNER 变），回调里读的是**旧属主** |
| `reorderFirst` / `reorderAfter` | 同一属主下成员次序变 |
| `attChange(el, attId)` | 某个属性变了 |
| `ruleChange(el, ruleId)` | 某条规则变了 |
| `insertInSecondaryList` / `removeFromSecondaryList` | 次层级（owner 链之外）归属变 |

### L2 的底层原语：属性怎么算「变了」

`DB_Element::attributesChangedBetween`（`0x5928100`）：

```
if !hasElementChangedBetween(el, s1, s2): return false           # 门
for att in el.getAtt(ATT_ATTLIS):                                # 枚举该元素的属性清单
    if hasAttributeChangedBetween(el, s1, s2, att): out.push(att)
```

`hasAttributeChangedBetween`（`0x593b930`）**按 `DB_Attribute::type()` 分派，逐类型比值**，
每个属性都要 `switchToOldSession` 到两端各读一次：

| type | 比法 |
|---|---|
| 1 / 6（size 1） | 整数（UDA ityp 51/52 先过 `DB_Uda::oldToNew` 归一） |
| 2（size 1） | double |
| 3（size 1） | bool |
| 4 | 字符串 `StrEq` |
| **5（size 1）** | **`DB_Ref::operator==` —— 指针属性（OWNER / SPRE / CATR …）** |
| 5（size≠1） | ref 数组 |
| 7 / 8 / 9 | `D3_Vector` / `D3_Point` / `D3_Matrix` 的 `!=` |
| 11 / 12 | 段 / 其它 |
| `supportsCases()` | 两端 `dabGetAtt` 比缓冲区 |

外加一个 `a8` 开关：为真时把「UDA 未设置」与「UDA 设为 0」区分开（走 `isUdaUnset`）。

`hasElementChangedBetween`（`0x593d6d0`）里有一条**硬编码特例**，直接对着我们的 P0-2：

```
if el.contentType == NOUN_TUBI:
    changed = POS变 || ORI变 || ITLE变 || SPRE变       # 隐式管身：只认这四个属性
else:
    changed = <dabacon 记录级 verb 16>
```

### L3 语义账 —— `DB_UserChanges`

`DB_DB::elementsChangedBetween(…, DB_UserChanges&)`（`0x58ffc50`）是 checkEle 的轻量同构版：

```
for el in <索引 modified 候选>:
    for att in attributesChangedBetween(el):
        if att == ATT_OWNER:
            switchToOldSession(BASE); elementIncluded(el, el.owner())     # 旧属主
        else:
            attributeModified(el, att)
    if noun.primaryList():
        <成员序差分> → elementReordered(member);  attributeModified(el, ATT_MEMB)
for el in elementsDeletedBetween(...):  elementDeleted(el)
for el in elementsInsertedBetween(...): elementCreated(el)
switchBackSession(); DBPlugger::ClearCaches()
```

两条要点：

- **`ATT_MEMB`：成员表变化被表达成属主身上的一个属性变化。** 容器加/删/换成员，
  在账上就是容器自己 `attributeModified(容器, ATT_MEMB)`，不需要另造概念。
- **`elementIncluded(el, oldOwner)`（`0x5987ea0`）把元素、旧属主、新属主三个都记进账**
  （属主自己就是新建的则跳过）——改挂时新旧两边的容器都进了变更集。

祖先抢占在 `DB_RawChanges` 层：`isElementTopLevelCreate`（`0x5983b80`）=
「我的属主**不在**新建集合里」，删除侧同理。整棵新建/删除的子树只上报最顶那一个。

### L4 消费者上卷 —— 「模型单元」是字典字段，不是手写名单

三个函数、两条实现路径，**判据同源**：

- `DES_DrawList::getAncestorTOPFElement`（`0x10515fc0`）：谓词
  `DB_PredicateElementTypeFieldEqual<bool>(661628, true)`，沿 owner 链上爬，爬不到就退回自身。
- `DES_DrawList::getChildTOPFElements`（`0x105166d0`）：**同一个谓词**，沿逻辑树下行，
  再滤掉 `isGraphicsIgnoredBetween`。
- `DES_DrawListManager::findTopLevelElement`（`0x1052c450`）→ `desdblib/FNDTOP`（`0x10380E38`）
  → `nounlib/LISTOP`（`0x561c93c`）。

**`661628` = `DB_Noun::toplevel`**（core.dll 里有具名访问器 `?toplevel@DB_Noun@@QBE_NXZ` @ `0x58dba70`；
仓库 `teach/learning-records/0010` 早就记过这个字段号）。

`LISTOP` 完整反编译后（两个字段常量由 `ida_bytes.get_dword` 实读 `0x5DB6D4C` / `0x5DB6D50`
得到 **661628 = toplevel**、**659518 = primitive**）：

```
LISTOP(noun):
    if noun.toplevel:
        if noun == WORL: return true
        return owner_noun != TMPL                      # 属主是 TMPL 时不算到顶
    if noun.primitive:
        if noun in {RPLA, PVOL, DRAW}: return owner_noun in {SITE, ZONE}
        if noun == FIXING: 上爬过所有非 toplevel 后，return 该 noun == STRU
    return false
```

`FNDTOP` 在 LISTOP 之上再加三条：`TUBI` 跨过 `BRAN` 那一级；爬到 `WORL` = 没有顶层单元；
到顶后若属主是 `TMPL` 则改取 `TMPL`。

**两个字段号在我们仓库里都已经是常量**：
`old-parse-pdms-db/src/dict.rs:68 FIELD_TOPLEVEL = 661628`、`:38 FIELD_PRIMITIVE = 659518`。
`primitive` 已经导进 `noun_flags.json`，**`toplevel` 没有**——这是全套里唯一缺的一格数据。

### L4 的两个入口，判据不同（这条纠正上一手的 A7/A8）

| 入口 | 场景 | 几何变没变的判据 |
|---|---|---|
| `changesBetweenSessions`（`0x1052b730`） | 显式比两个会话端点 | **没有额外判据**，三类候选一律 `findTopLevelElement` 后上账 |
| `PostLocalChanges`（`0x1052aea0`），`op == 4` | GETWORK 后刷新 | 新建无条件；**修改经 `hasTopLevelGraphicsChanged` 过滤** |

`hasTopLevelGraphicsChanged`（`0x1052c850`）**全模块只有一个调用方**，就是
`PostLocalChanges+0x1aa`（`0x1052b04a`）——xrefs 实查，唯一 call。它的判据（指令流实读
`0x1052c917`–`0x1052c92d`）确实是「`attributesChangedBetween(当前会话, 上一会话)` 的结果里有没有 `ATT_CACHID`」。

→ **上一手 b8ce34bd 的 A7/A8 把 CACHID 归到了两会话差分路径上，归错了。**
我们做的是 `changesBetweenSessions` 那个场景，**那条路径根本不看 CACHID**。
b9fc69a5 实测「设计库 BRAN 记录里 CACHID `encoded_location=0`、取不到值」与此不矛盾，
也不再构成阻塞——**CACHID 这条整个不必对齐**。

`PostLocalChanges` 另外两条可用规则：`DB_DB::type() == 1` 过滤（= R1「只处理 DESI」的代码级门，
不是命令行参数），以及 `DB_RawChanges::trim` 按库裁剪。

`processCreates`（`0x1052cc10`）与 `processModifications`（`0x1052cf80`）反编译后**形状完全一致**：
都是 `findTopLevelElement` + 插入。→ **A5 坐实：三类候选全部上卷，新建也不例外。**

---

## 2. 与 e3d-model 现状的逐层对照

| 层 | core | e3d-model 现状 | 判定 |
|---|---|---|---|
| L0 会话定位 | `switchToOldSession` | 两个各钉 sesno 的 `DbSet` | ✅ 对齐 |
| L1 索引候选 | `DB_IndexTableCompare` 三态 | `IndexDiff` → `IndexCandidate` 三态 | ✅ 对齐 |
| **L2 逐元素分类** | **`checkEle` 12 回调 + 逐属性按类型比值** | **完全没有** | ❌ **整层缺失** |
| **L3 语义账** | **`DB_UserChanges` 8 桶 + 祖先抢占** | **完全没有** | ❌ **整层缺失** |
| L4 上卷判据 | `noun.toplevel` 字典字段 + LISTOP/FNDTOP 特例 | 手写 `Category::is_positive_solid` | ❌ 判据不同 |
| L4 三类上卷 | 创建/删除/修改都上卷 | 删除的正体不上卷（照了 `PartialUpdateDesiMgr` 的 R15） | ❌ 照错机制 |
| L4 顶层已消失 | `getChildTOPFElements` 问已画出的那份 | 无「已产出模型集」输入 | ❌ 缺 |
| L4 库过滤 | `DB_DB::type()==1` 代码门 | `--db-type DESI` 命令行 | ◐ |

e3d-model 现在的 `plan_update`（`increment.rs:312`）实际是把 **L1 直接接到 L4**，
中间缺掉的 L2/L3 用两个几何启发式顶替：`placement_drifted`（比两端 POS/ORI）+
`collect_positive_subtree`（变换级联）。**这两个函数在 core 里没有任何对应物**——
它们是为了补偿缺失的属性级差分而发明的。

---

## 3. 目标架构（按 core 的方式）

```
crates: e3d-io（L0/L1 已有）  →  e3d-model::increment（本次改造 L2/L3/L4）

  ┌ L0 两端 DbSet（已有）
  │
  ├ L1 IndexDiff → IndexCandidate{Inserted|Modified|Deleted}          （已有，保留）
  │
  ├ L2 ElementDiff ───────────────────────────────── 新增
  │     对每个 Modified 候选：
  │       ① 门：element_changed(el)     —— TUBI 走 (POS,ORI,ITLE,SPRE) 特例
  │       ② 枚举该元素属性清单，逐属性按类型比两端值
  │       ③ 产出 ChangedAttrs{ Vec<AttrId> }
  │     Inserted / Deleted 不做属性差分（整体处置）
  │
  ├ L3 ChangeLedger ─────────────────────────────── 新增
  │     Created{el}                     ← Inserted 且属主不在 Created（祖先抢占）
  │     Deleted{el}                     ← Deleted   且属主不在 Deleted（祖先抢占）
  │     Reparented{el, old_owner, new_owner}   ← ChangedAttrs 含 OWNER
  │     MembersChanged{container}       ← ChangedAttrs 含 MEMB
  │     Reordered{el, container}
  │     AttrsModified{el, attrs}        ← 其余
  │     TypeChanged{el}                 ← noun 变 → 等价于 Deleted + 整子树 Created
  │
  └ L4 UnitPlan ─────────────────────────────────── 改判据
        top_level_of(el) = 沿 owner 链找 noun.toplevel（LISTOP/FNDTOP 三条特例）
        三类账目全部上卷 → BTreeSet<RefNo> 去重
        顶层在 target 端已消失 → 问「已产出模型集清单」要该子树下产过哪些单元 → Remove
        → upserts / removals
```

### 3.1 `toplevel` 数据怎么来 —— 以及**为什么它不能替换 `is_model_unit`**

> **实测修正（2026-08-31 19:4x，b9fc69a5）。** 本节原写「补进 `noun_flags.json`，
> 然后 L4 把单元判据换成 `noun.toplevel`」。数据取回来之后，**后半句是错的**，
> 前半句的路径也绕远了。原文保留在下面的「原计划」里，结论以本节为准。

**取数路径（比原计划短）**：`scripts/noun_family_probe.py` 已经持有 `AttrDataFile`，
并且已经在用 `df.raw_field(noun_hash, FIELD_POSITIVE_EQUIVALENT)` 直读任意字段。
所以加 `toplevel` **不需要动 `old-parse-pdms-db`、不需要重导 `noun_flags.json`**：
探针里加一行 `df.raw_field(h, 661628) == 1`，重跑一次 `data/noun-family-matrix.json` 即可。
那份 JSON 正是 e3d-model 用 `include_str!` 内嵌、且 `tests/noun_coverage.rs` 已在消费的表。

原计划指的 `dict.rs:1111` 导出器产出的是 `noun_flags.json`，
而 e3d-model **根本不读那个文件**（只读 `data/route-nouns.json` 与
`data/noun-family-matrix.json`）——`noun_flags.json` 只是探针用来把 hash 翻成短名的中间物。

**实测分布（3.1 字典 `shadow_e3d31_aps_all\attlib.dat`，1931 个 noun，211 个 `toplevel=1`）**：

| noun | `toplevel` | `is_model_unit` |
|---|---|---|
| `BOX` `CYLI` `REVO` `POLYHE` `AEXTR` | **false** | **true** |
| `SBFI` | **false** | **true** |
| `PANE` `GWALL` `FLOOR` `SCTN` `WALL` `STWALL` | true | true |
| `EQUI` `STRU` `TMPL` | **true** | **false** |
| `BRAN` `HANG` `LUG` `SUPC` `TRUNNI` | **true** | **false** |
| `TUBI` `FTUB` `SITE` `ZONE` `WORL` `FIXING` | false | false |

**两个集合互不包含。** 这不是数据错，是**两层不同的粒度**：

- `toplevel` = **可绘单元**。core3d 的 draw list 一条就是一整件 `EQUI`，
  它底下那些 `BOX` 不单独成条，所以 `BOX` 不必是 toplevel。
- `is_model_unit` = **网格产出单元**。我们一个 `BOX` 出一件网格、挂在它自己的 refno 上。

把 `is_model_unit` 换成 `toplevel` 会同时坏两头：

1. 一个 `BOX` 改了就得重建整件 `EQUI`——把重建范围凭空放粗一个数量级，
   而我们的产物粒度根本没有「一件 EQUI」这个东西；
2. 五个路由容器全是 `toplevel=true`，会被重新拉进来——那正是 `is_model_unit`
   **有意排除**的一批（Remove 按 refno 发，容器 refno 上没有一件几何可删，
   隐式管身按 `GeometryId::ImpliedTube` 索引）。

**所以 L4 不是「换判据」，`toplevel` 是要新加的一层更粗的键，不是替换现有的那层。**

**顺带解开一个死结**：`BRAN`/`HANG`/`LUG`/`SUPC`/`TRUNNI` 全部 `toplevel=true`，
而隐式管身在概念上就属于它的容器——这说明「RouteContainer 的 Remove 无键可发」
和「引入 toplevel 层」**是同一件工作**：容器就是那条管身的 toplevel 键。
两件事该合并做，不该像原计划那样排成先后两项。

<details>
<summary>原计划（已被上面的实测推翻，留档对照）</summary>

`old-parse-pdms-db/src/dict.rs` 里 `FIELD_TOPLEVEL = 661628` 已定义，
导出器 `dict::tests::export_noun_flags_json`（`dict.rs:1111`，`#[ignore]`）少导这一列。
改动量：`NounFlags` 加一个 `toplevel: Option<bool>`、导出器加一行、重跑一次导出。
`primitive` 已在表里，LISTOP 需要的两个字段就齐了。

</details>

### 3.2 P0-1（属主变更漏级联）：L2 抓得到，但级联**删不掉**

> **落地修正（L2/L3 实装后回填）。** 本节原写「问题自动消失、变换级联可以整体删掉」，
> 前半句成立、后半句是错的。留原判断在此并改正，理由见下。

成立的部分：改挂在 core 里就是 `OWNER` 变了，L2 差分必然抓到，L3 记成 `Reparented`，
旧属主与新属主两侧都进账（`ElementDiff::owner_changed` + 两侧的成员表变动，
真库门 ams8000 45→46 实测 `reparented=1 members=2`）。P0-1 的**检出**确实由 L2 兜住，
不再需要 `placement_drifted` 那条只比 `(POS, ORI)` 的启发式。

错的部分：**`collect_positive_subtree` 不能删。** core3d 删得掉是因为它的 draw list
按元素存局部变换、由渲染期沿层级折叠，祖先动了后代不必重建；e3d-model 产出的是
**世界系网格**，祖先的矩阵已经烘进后代顶点里了。后代此时在索引里往往一条记录都没动
（真库门 ams8000 195→196 实测：`cascades=1`，其中 1 件后代**索引记录完全没变**，
只有级联捞得回来）。删掉级联 = 世界系产物直接错位。

所以落地形态是：级联保留，但**触发判据从几何启发式换成 L2 事实**——
`ElementDiff::placement_input_changed()` = `POS`/`ORI` 任一变 ∨ `owner_changed`
∨ `type_changed` ∨ `opaque`。`PLACEMENT_ATTRIBUTES` 由 `transform.rs` 单点导出，
与矩阵折叠共用一份属性名，判据与消费者不会再各自漂。

顺带解决 P2 的性能：ams1112 那 24674 个候选里，绝大多数是 `Deleted`（不做属性差分），
真正要逐属性比的只有 `Modified` 那一小撮；`PROBE_MAX_CANDIDATES=4000` 这个绕过可以撤。

**语料证不到的部分（不许当已证）**：全部 443 个库扫下来只有 22 个改挂窗口，
22 个的本级 `(POS, ORI)` 全都没动（老判据的盲区是真的），但被改挂的要么自己就是
模型单元（索引已点名，target 端上卷照样重建），要么上下都没有单元（两种实现都不产动作）。
**没有一个窗口能把修好的实现和没修的区分开。** P0-1 的修复正确性目前靠的是
「世界矩阵沿 owner 链折叠」这条构造性事实 + 谓词单元测试，不是真库门的输出差异。

### 3.3 P0-2（隐式管身）拿到 core 的现成答案

`hasElementChangedBetween` 对 `NOUN_TUBI` 硬编码 `(POS, ORI, ITLE, SPRE)`；
`FNDTOP` 让 TUBI 上卷时跨过 BRAN 那一级。这两条直接抄，`route.rs` / `catalogue_point.rs`
落地时不必重新发明主键规则。

### 3.4 目录跨库闭包：core 也不靠 owner 闭包

`SPRE` 是 type 5 的指针属性，`hasAttributeChangedBetween` 用 `DB_Ref::operator==` 比它。
即：**设计件重新指向另一个目录件，core 抓得到**（元件自己的 SPRE 变了）。
但**目录件自身内容变了**，core 在这条路径上同样抓不到——它靠的是目录库改版时
`PostDBFileChanges` / `ClearCaches` 全清缓存。这条给我们的启示是：
跨库闭包不该做进窗口差分，应该做成**「目录库版本变 → 该目录件的所有引用者整体失效」**
的独立通道。这也说明 grilling 第一问选项 D 的形状本来就不对。

### 3.5 「已产出模型集」是必需输入

`getChildTOPFElements` 的存在说明：当顶层元素在 target 端已经不存在时，
**库里问不出来它下面曾经有过什么**，只能问自己的产出。
`execute_plan` 现在只吃 target 端 `DbSet`，必须补一个产出清单（refno → 上次产出的单元集）。
这同时是「删除侧多出 8 件」那类账目模糊的根治办法。

### 3.6 我们的 CACHID 等价物

core 在 GETWORK 路径上用 `ATT_CACHID` 做「几何真的变了吗」的快筛。
我们既读不到它（b9fc69a5 实测无存储位），也不需要它：
我们自己就是几何的生产者，可以对每个单元算一个**几何输入摘要**
（该单元实际消费到的属性集合的哈希）。这比 CACHID 更强——CACHID 只是 core 的缓存标识，
而摘要能精确回答「重算了但结果一样」，直接压掉无谓的 upsert。
**这是本设计里唯一一处有意超出 core 的地方，且方向与主计划 §1.2 的实例化产物形态一致。**

---

## 4. 遗留缺陷在新架构下的归宿

「今日实测」列由会话 `f40baab7` 于 2026-08-31 21:0x 逐条回源码复核，行号对
`e3d-model` 提交 `491d0e3`。

| 缺陷 | 归宿 | 今日实测 |
|---|---|---|
| P0-1 属主变更漏级联 | **已修**（OWNER 进 L2，级联判据改吃 L2 事实）；语料无判别性窗口，见 §3.2 | ✅ 已落地。`placement_drifted` 已删，`increment.rs:560` 吃 `placement_input_changed()` |
| P0-2 隐式管身 | 抄 core 的 TUBI 特例（`hasElementChangedBetween` + `FNDTOP`） | ⬜ 未动。口子仍只由 `stale_route_containers` 计数 |
| `contributed \|\| fanout > 0` 账目说谎 | 级联留着（世界系产物必需），账目改由 L3 守恒兜，见 §3.2 | ⚠️ **仍在**（`increment.rs:571`）。而它指望的「L3 守恒」还没建，见下一行 |
| `flag_only_drifts` 只记不判 | 保留为观测量；判据已由 `placement_input_changed` 接管 | ✅ 按此执行，不算欠账 |
| `accounts_for` 恒等式不全 | 重写为 L3 账目的守恒（每条账都必须有归宿：上卷成功 / 无 toplevel 祖先 / 显式豁免） | ⬜ 未动。`accounts_for`（`increment.rs:213`）仍只有候选账、执行账两条，`ChangeTally` 一个字段都不参与判定。**且账本身就漏**：`ledger.created / deleted / record` 三个入口全在 `Ok(...)` 分支里，上卷失败与差分失败的候选不进账——守恒式要成立得先补 unresolved 侧的记账 |
| ~~`is_positive_solid` 手写名单 → 换成 `noun.toplevel`~~ | **本行作废**，被 §3.1 的字典实测推翻：`toplevel`（可绘单元）与 `is_model_unit`（网格产出单元）两个集合互不包含，不是替换关系 | ✅ 已改按 §3.1：`is_model_unit` 改写成无通配符的穷举 `match classify(noun)`，加 `Category` 变体当场编译不过 |
| ~~删除的正体不上卷（照错 R15）→ 改成三类全上卷~~ | **本行作废**，当初判错：单元自己被删就该直接 Remove，不该再上卷；非单元的删除已经在上卷到属主单元 | ✅ 现行 `increment.rs:475` 的分支是对的，无需改 |
| 真库门 `eprintln` + `return` 照样绿 | 独立于本设计，仍需修（缺件必须红） | ⚠️ **仍在**（`tests/increment_real.rs:149`）。本机 fixtures 齐全所以真的跑了 23.7 s，换台没有 `E:\` 的机器整道门静默变绿 |
| 大窗口无成本护栏 | 属性差分只对 `Modified` 做，量级下来了；仍需保留超阈值退化策略 | ⬜ 未动。**另有一条本表原先没记**：`collect_unit_subtree` 的 `visited` 每次调用新建（`increment.rs:392`），一窗内多个级联把重叠子树重复读 N 遍 |

---

## 5. 未验 / 不许当已证

1. `DB_Compare::scanOld` / `scanNew`（`0x5a46a40` / `0x5a46730`）内部未读——
   `checkEle` 的调用序、以及删除侧具体怎么被枚举出来，是从 `dealWithChangeType` 与
   vtable 槽位回填推的，**没有逐指令验**。
2. `checkEle` 里 `sub_58E8090` / `this+20` 那个「位置变了」判据，只确认了它触发
   `reorder*` 分支，**判据本身没拆**。
3. vtable 槽位到 `DB_RecordCompare` 具名方法的映射，是靠 `attChange`（槽 10）、
   `includeAfter`（槽 8）、`createAfter`（槽 3）、`deleteAll`（槽 2）四个锚点回填的，
   `createFirst` / `createNull` / `reorder*` / `*SecondaryList` 六个**按位置推定，未逐个反编译确认**。
4. `noun.toplevel` 到底有多少个 noun 为真 —— **字典里还没导出过这一列**，
   与 CACHID 那 204 个 noun 是否一致未知。这是落地第一步就要拿到的数据。
5. `LISTOP` 里 `FIXING`（108608856）那条上爬循环的终止条件，读的是伪码，未逐指令验。
6. `isGraphicsIgnoredBetween`（`0x1051c7d0`）内部未读——它可能就是 R2「XGEOMETRY 子树排除」的落点。
