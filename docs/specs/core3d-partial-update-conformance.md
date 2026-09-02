# 核对表：模型增量更新 vs Core3D `PartialUpdateDesiMgr`

> 对象：`D:\AVEVA\Everything3D3.1\Core3D.dll`（32 位，ida-bridge `idalib-32268`）
> 取证：`docs/evidence/2026-08-27-ida-core3d-partial-update-model-impact.md`（叙述体）
> 计划：`docs/plans/2026-08-27-model-impact-core3d-parity-plan.md`
> 用例：`docs/specs/core3d-partial-update-test-cases.md`
> 我方：`src/data_interface/{model_impact,generation_root,model_update_plan,update_scope}.rs`

## 0. 这份文档是干什么的

证据文档是**叙述**——它讲清楚了 core 怎么想。这份是**核对表**：把同一批事实拆成
**逐条可判真假的规则**，每条都带三样东西——core 的确切行为、能重新验证它的地址或 SQL、
我方今天的对应物。要么打勾，要么记一条缺口，不留"大概是这样"。

三种用法：

1. **改代码前**：找到你要动的那条规则，先看 core 到底怎么做，再决定抄不抄。
2. **改代码后**：把该条的"核对办法"跑一遍。每条规则在
   [用例集](core3d-partial-update-test-cases.md)里都有一个编号对应的用例。
3. **换版本后**（E3D 升级）：§1 的地址表整个失效，但**规则编号不变**——
   按 §1 的重定位办法把地址重取一遍，规则逐条重验。

判定符号：✅ 已对齐 ｜ ◐ 形状相同判据不同 ｜ ❌ 缺 ｜ ⚪ core 侧不成立或不可比

> **本轮（2026-08-28）用 ida-bridge 复核指令流，改了证据文档三处、新增六条它没有的规则。**
> 更正见 §7，逐条在正文里标 `★更正` / `★新增`。

## 1. 对象与地址（重定位的起点）

### 1.1 单例字段布局

| 偏移 | 名字 | 类型 | 读出处 |
|---|---|---|---|
| `+0x05` | `m_inDraw` | bool | `Update` 重入保护；pass 2 置 1，`Finish` 后清 0 |
| `+0x08` | `m_view` | `DB_Element`(20B) | 当前视图，`SetView`/`ResetView` |
| `+0x1C` | `m_enabled` | bool | `UpdateOn` |
| `+0x1D` | `m_muted` | bool | `Mute` / `UnMute` |
| `+0x20` | `m_granularityMode` | int | **恒 0**，全类无 setter（§4.8） |
| `+0x24` | `m_queue.begin` | ptr | 待办队列 |
| `+0x28` | `m_queue.end` | ptr | |

### 1.2 队列记录布局（stride 24 字节）

| 偏移 | 内容 |
|---|---|
| `+0x00` | `DB_Ref`（12 字节） |
| `+0x0C` | 元素句柄第 3 字 |
| `+0x10` | 元素句柄第 4 字 |
| `+0x14` | `ModelState`（int） |

push 走 `sub_1047BA50`。stride 证据：`IsPresent` 的 `v4 += 24`、`AncestorDeletes` 的
`add esi, 18h`（`0x1047C155`）。

### 1.3 方法地址（E3D 3.1，`Core3D.dll`）

| 地址 | 方法 | 角色 |
|---|---|---|
| `0x1047BA50` | `push_back(Record)` | 队列写入 |
| `0x1047BC40` | 构造函数 | 唯一写 `+0x20 = 0` 的地方 |
| `0x1047BE10` | `AbsentPrimitives` | 缺失图元回收 |
| `0x1047BF80` | `AddIDList` | 把元素并进视图 ID 清单并写库 |
| `0x1047C060` | `AncestorDeletes` | 删除类的祖先标记 |
| `0x1047C200` | `ChangedModelToUpdate` | 入口：消费 QCHGLS |
| `0x1047C2E0` | `DeletedModelToUpdate` | 入口：删除（别名 `DeletedModelNotify`） |
| `0x1047C350` | `DrawModel` | 发 PML `PUPDES` |
| `0x1047C670` | `EraseModel(DB_Element&)` | 按 significant 位分派 |
| `0x1047C6D0` | `EraseModel(DB_Ref&)` | 按队列里有无 state 2 分派 |
| `0x1047C720` | `ErasePrimitive(DB_Element&)` | |
| `0x1047CB80` | `ErasePrimitiveFromCandidateModel` | |
| `0x1047CCF0` | `ErasePrimitiveFromUnknownModel` | |
| `0x1047CF00` | `EraseSignificant(DB_Ref&)` | |
| `0x1047D050` | `Exists` | "还有没有可画的东西"（递归） |
| `0x1047D230` | `ExistsCPS` | 查已绘制段 |
| `0x1047D380` | `Finish` | 清空队列 + 通知 |
| `0x1047D650` | `GetIDList` | 取视图 ID 清单 |
| `0x1047D8C0` | `GranularityExpansion` | **粒度中心算法** |
| `0x1047DC00` | `IsAncestorDeletesPresent` | 队列里有无 state 2 |
| `0x1047DC30` | `IsDecendantPresent` | 队列里有无该 state 的子孙 |
| `0x1047DD20` | `IsDeletesPresent` | 队列里有无 state 3 |
| `0x1047DD50` | `IsManagedStructure` | XGEOMETRY 判定本体 |
| `0x1047DD90` | `IsNegative` | 字段 `0x92663`，int ≠ 0 |
| `0x1047DDD0` | `IsPending` | **去重中心算法** |
| `0x1047E020` | `IsPresent` | 队列里有无 `(elem, state)` |
| `0x1047E070` | `IsPrimitive` | `0xA103E` ∨ `0xBBD5ADC` |
| `0x1047E0D0` | `IsSignificant` | `0x5657A0A` |
| `0x1047E240` | `Members` | 三模遍历 |
| `0x1047E590` | `ModelToUpdate` | **内部总入口** |
| `0x1047E650` | `NegativeMembers` | `Members(mode=2)` |
| `0x1047E670` | `NewModelNotify` | 入口：新建（无 ID 清单副作用） |
| `0x1047E6E0` | `NewModelToUpdate` | 入口：新建（**有** ID 清单副作用） |
| `0x1047E7E0` | `PrimitiveMembers` | `Members(mode=1)` |
| `0x1047E800` | `Refresh` | **清空整个队列** |
| `0x1047E9C0` | `SignificantMembers` | `Members(mode=0)` |
| `0x1047E9E0` | `SignificantOwner` | 含自身的向上攀爬 |
| `0x1047EAB0` | `Update` | 三遍消费队列 |

### 1.4 换版本后怎么把地址取回来

方法名带完整修饰名，符号在导出表里，所以不用靠特征码：

```powershell
ida-bridge exec <client> --sql "SELECT start_ea, name FROM funcs WHERE name LIKE '%PartialUpdateDesiMgr%' ORDER BY start_ea"
```

字段 id 靠字节搜（`push imm32`）：

```powershell
# 0x5657A0A significant / 0xA103E primitive-A / 0xBBD5ADC primitive-B / 0x92663 negative
ida-bridge exec <client> --sql "SELECT '0x5657A0A' AS id, address, name_at(func_start(address)) AS func FROM bin_search WHERE pattern = '68 0A 7A 65 05' UNION ALL SELECT '0xA103E', address, name_at(func_start(address)) FROM bin_search WHERE pattern = '68 3E 10 0A 00' UNION ALL SELECT '0xBBD5ADC', address, name_at(func_start(address)) FROM bin_search WHERE pattern = '68 DC 5A BD 0B' UNION ALL SELECT '0x92663', address, name_at(func_start(address)) FROM bin_search WHERE pattern = '68 63 26 09 00'"
```

> **Hex-Rays 在这个类上系统性出错，凡涉及判据的函数一律读指令流。**
> `getField(id, &out)` 的出参它认不出来，于是把调用渲染成无参、把 `out` 的比较整个丢掉。
> 后果不是"少了个参数"，是**丢分支**：`GranularityExpansion` 的整条 significant 分支
> （`0x1047D91A` 的 `jz`）在伪码里根本不存在；`Exists` 丢了递归那一半；`IsNegative` 直接反编译失败。
> 判定这类函数是否被坑，看伪码里有没有孤零零的 `DB_Noun::getField(vN);` 后跟 `return 0`。

## 2. 判据：两个 bool 位

| 判据 | 地址 | 字段 id | 取值 |
|---|---|---|---|
| `IsSignificant(e)` | `0x1047E0D0` | `0x5657A0A` / `90536458` | bool |
| `IsPrimitive(e)` | `0x1047E070` | `0xA103E` / `659518`，为假再试 `0xBBD5ADC` / `196958940` | bool ∨ bool |
| ~~`IsNegative(e)`~~ | `0x1047DD90` | `0x92663` / `599651` | int ≠ 0，**本链无活的消费者** |

**R0-1 取值走出参，不走返回值。**
三个判据都是"把栈上 out 清零 → `getField(id, &out)` → 读 out"，`getField` 自己的 bool 返回值
（"这个 noun 登没登记这个字段"）被丢弃。按 core 的口径，**字段未登记 = 该位为假**。
`IsPrimitive` 两次调用共用一个 out 且只清零一次——安全，因为第一次为真就不会有第二次。

- 核对：`SELECT address, disasm FROM instructions WHERE func_ea = 0x1047e070 ORDER BY address`
- 我方：✅ 已导（P0，2026-08-28）。`tests/fixtures/core-noun-granularity-e3d31.json`，
  通道是 `core.dll!DB_Noun::getField(id, &out)` + `findNoun(hash)`，照 core 的口径清零取出参。
  1931 个 noun 三张表 unknown / not_found 全为 0，所以"未登记 = 该位为假"这条在本快照上
  没有被触发过。细节见 `docs/evidence/2026-08-28-core-noun-granularity-export.md`。

**R0-2 `primitive` 不是跨版本固定谓词。**
`0xA103E` 稳定；它的搭档会换——2.10 的 `MassProperties::PopulateCSGtree` 配 `0xA18B8`，
而 `0xA18B8` 在 3.1 里一次都搜不到。快照必须**分开存两位并记来源版本**，
不要在导出时就合成一个 `primitive` 布尔。

- 我方：✅ 两位分开存（快照 `fields` 下就是 `significant` / `primitive_a` / `primitive_b`
  三个键，没有合成键），来源版本由 `core_sha256` 钉住。生产侧的查询
  `generation_root::core_primitive_bits` 也返回 `(a, b)` 而不是一个布尔，
  `noun_is_primitive` 才取或——想知道"是哪一位说了算"永远问得出来（T2a）。

## 3. 入口与门（R1–R8）

### R1 只处理 DESI 库 ✅

`DB_DB::type(e->getDB()) == 1`，在 `ModelToUpdate` 里（`0x1047E590`）。
我方等价物：`UpdateScope::admits(db_type, dbnum)`。

### R2 XGEOMETRY 子树整体排除 ❌

`DB_Element::climb(e, NOUN_XGEOM).isNull()` 必须为真才继续。
凡是有 XGEOMETRY 祖先的元素，core **一律不进**局部更新——显式几何走另一条路。
`NOUN_XGEOM` 取自全局（`0x1047DD5E`），字典短名 `XGEOM`，hash `7739277`
（`output/noun_layout.json:11666`）。判定本体是 `IsManagedStructure` `0x1047DD50`。

`NewModelToUpdate` / `NewModelNotify` / `DeletedModelToUpdate` 三个入口**自己先查一遍**，
`ModelToUpdate` 再查一遍——这是唯一被重复检查的门。`ChangedModelToUpdate` 不查：
它的每一条都来自 QCHGLS，逐条转交 `ModelToUpdate` 时才过门。

- 我方：**无**。计划 T2.2。

### R3 元素必须 `isValid` ★新增 ◐

`ModelToUpdate` 有一道 `DB_Element::isValid(e)`，四个公共入口都没有。
证据文档 §3 的门清单漏了这条。

- 我方：◐ 我们从持久层取 `pe`，取不到就 `None`，形状等价但不是同一件事——
  core 判的是**元素句柄有效**，我们判的是**库里有没有这一行**。

### R4 三个开关的检查顺序 ★更正 ✅

证据文档 §3 把顺序写成"enabled → view → muted → XGEOM → DESI"，实际是：

```
ModelToUpdate(e, state):
    if !m_enabled                      return    # +0x1C
    if !m_view.isOK()                  return    # +0x08
    if !e.isValid()                    return    # ★ R3
    if e.getDB().type() != 1           return    # R1
    if !climb(e, NOUN_XGEOM).isNull()  return    # R2
    if m_muted                         return    # +0x1D
    if IsPending(e, state)             return    # R17–R20
    GranularityExpansion(e, state)
```

语义上无差别（全是无副作用的早退），但按这份顺序排查线上问题才对得上。

### R5 QCHGLS 的 change code 被丢弃 ✅

`ChangedModelToUpdate`（`0x1047C200`）按**步长 3** 遍历 `change/QCHGLS`，
每条 `HGETIA(..., count=2)` **只取前两个字**（`DB_Ref`），第三个字（change code）不读。
每一条都当 `Changed(0)` 处理。

**推论**：DCHC 码的全部作用发生在上游——决定元素**进不进** QCHGLS；
一旦进了，重画范围只由 noun 位决定。**core 这一层没有 `TransformOnly`。**

- 我方：✅ 分层等价。`OperationEffectSummary.max_dchc` 保留原始码没问题，
  但不该让它调节重生成范围。`TransformOnly` 是我们**主动多留**的便宜路径（省不是漏），
  按计划 §1 不取消。

### R6 `Refresh(当前 VIEW)` 清空整个待办队列 ★新增 ❌

```
Refresh(e):                                     # 0x1047E800
    if e.type() == NOUN_VIEW and e == m_view:
        m_queue.end = m_queue.begin             # 整队丢弃
```

视图自己被刷新时，所有排着的增量工作项**作废**——反正整个视图要重画。

- 我方：**无**。我们做整库重建 / 全量重生成时，`model_update_pending` 里的待办
  不会因此清空，可能在重建后又跑一遍已经不需要的增量。**这是一条真缺口**，
  而且比 P3.3 那种"多做"更值钱——它是"做无用功 + 可能覆盖新结果"。

### R7 新建元素会被并进视图 ID 清单并写库 ★新增 ⚪

```
NewModelToUpdate(e):                            # 0x1047E6E0
    ...门...
    if !IsPending(e, New):
        idlist = GetIDList()
        if idlist.active and !idlist.exists(e):
            owner = SignificantOwner(e)
            if owner.isOK(): AddIDList(owner)   # 0x1047BF80 → addElement + writeDB
    ModelToUpdate(e, New)
```

`AddIDList` 会 `PDMS_Idlist2::writeDB()`——**这是一次落库副作用**。

另有 `NewModelNotify`（`0x1047E670`）走同一个 state 1 但**没有**这段副作用。
证据文档 §1 只列了 `NewModelToUpdate`，把这两个入口混为一谈了。

- 我方：⚪ 不可比——ID 清单是 E3D 视图的绘制清单，我们没有对应概念。
  **但它决定了 R22 的判据**（见下），所以移植 `AbsentPrimitives` 时必须先想清楚
  "我们这边什么东西扮演 ID 清单"。

### R8 `Update` 的重入保护 ★新增 ⚪

`Update` 入口检查 `m_inDraw`（`+0x05`）；pass 2 每次 `DrawModel` 前置 1，
`Finish` 之后清 0。因为 `DrawModel` 发的是 PML 命令，会同步回调进来。

- 我方：⚪ 我们的重生成不是同步回调，`model_concurrency.rs` 另有一套。不构成缺口，
  但说明**队列消费期间不能接受新的入队**这条约束在 core 侧是硬的。

## 4. 粒度：`GranularityExpansion`（R9–R16）

指令流还原（`0x1047D8C0`，伪码不可用——见 §1.4）：

```
GranularityExpansion(e, state):
    if IsSignificant(e):                              # 0x1047D915 cmp / 0x1047D91A jz
        push(e, state)                                # 0x1047D942
        AncestorDeletes(e, state)                     # 0x1047D94B
        AbsentPrimitives(e, state)                    # 0x1047D954
        for m in Members(e, Significant):             # 0x1047D97E  mode 0
            push(m, state == Deleted ? 3 : 4)         # 0x1047D9B4 cmp ebx,3 → var_14 = 3|4
            AncestorDeletes(m, state)                 # 0x1047D9C9
            AbsentPrimitives(m, state)                # 0x1047D9D2
        return                                        # 0x1047DA41 jmp 过尾部的 AncestorDeletes

    if !IsPrimitive(e):                               # 0x1047DA4E
        return                                        # 0x1047DA55 jz —— 什么都不做

    if m_granularityMode == 0:                        # 0x1047DA5B（恒真，§4.8）
        if state == Deleted: push(e, Deleted)         # 0x1047DA65 / 0x1047DA89
        else:
            owner = SignificantOwner(e)               # 0x1047DA99
            push(owner, state)                        # 0x1047DAC1
            AbsentPrimitives(owner, state)            # 0x1047DACD
    else:                                             # 死代码，0x1047DAD7
        push(Members(e, Negative) ? SignificantOwner(e) : e, state)

    AncestorDeletes(e, state)                         # 0x1047DBD3
```

**这一段证据文档 §6 写对了**，本轮从指令流逐跳复核通过。

### R9 粒度由 noun 位决定，不由类型名单决定 ◐

- 我方：**判定链仍是手写名单** `DEFAULT_DELIVERY_UNIT_TYPES`（BRAN/HANG/SUPPO/EQUI）+
  `COARSE_HIERARCHY_NOUNS` + `is_loop_container_noun`。P1 的对账已量化这个差
  （`docs/evidence/2026-08-29-noun-granularity-reconciliation.md`）：方向几乎全是多做，
  真漏只有 `AIDTEX` 一个 noun，故分叉点判为**加层**而不是换判据。
  T2a 已把位表接进生产（`noun_is_significant` / `noun_is_primitive`，快照外保守取真），
  **但还没有消费者**——第一个是 T2b。

### R10 significant 变化 → 整块重画 + 块内 significant 后代逐个抹掉 ❌

块内后代入队时的 state 是 `state == Deleted ? 3 : 4`，两者在 `Update` 里**都只 Erase 不 Draw**。
"块级颗粒"的确切含义：不是"重画子树里每一个"，是"**重画块，顺手删掉块内那些自己也曾单独成块的行**"。

- 我方：**无**。同一几何可能在模型里出现两份。计划 T3.2。

### R11 `SignificantMembers` 只穿过 significant 链 ★精确化 ❌

`Members(e, mode=0)`（`0x1047E240`）：种子是 `e` 的**直接成员**，用显式栈做 LIFO；
每弹出一个成员，**不 significant 就既不收集也不下潜**（`0x1047E37E` / `0x1047E381`）。

**推论（可判真假）**：非 significant 的子节点会**挡住**它下面的 significant 孙节点——
后者不会被收集。这不是遍历的副作用，是判据本身。

三个模式的完整规则：

| mode | thunk | 收集 | 下潜 | 指令 |
|---|---|---|---|---|
| 0 Significant | `0x1047E9C0` | significant 成员 | **只穿过 significant 成员** | `0x1047E37E`–`0x1047E3ED` |
| 1 Primitive | `0x1047E7E0` | primitive 成员 | **全部成员，整棵子树** | `0x1047E3F9`–`0x1047E46C` |
| 2 Negative | `0x1047E650` | primitive **且** negative | 只穿过 primitive **且非 negative** 的成员 | `0x1047E475`–`0x1047E4EC` |

mode 2 的"收集了就不再下潜"是本轮新看清的（`0x1047E48C jmp` 直接跳到下一轮）。
它挂在死代码上（§4.8），登记备查。

返回值是 `out` 非空。

### R12 primitive 且非 significant 的变化 → 上卷到 SignificantOwner ◐

并对该 owner 做 `AbsentPrimitives`。

- 我方：◐ `resolve_element_generation_root` 形状相同（MDU 优先 → 跨 loop 容器 →
  第一个非容器 owner → 自身），但判据是名单、且有深度上限 32。

### R13 既非 significant 又非 primitive → 完全丢弃 ◐

**连 `AncestorDeletes` 都不做**（`0x1047DA55` 的 `jz` 直接跳到函数尾）。

- 我方：◐ 我们是 `Unknown → Regen` 保守兜底，**多做不是漏做**。
  在两张位表拿到并验证完之前不能改成丢弃——那才是漏判。

### R14 `SignificantOwner` 含自身、无深度上限 ◐

```
SignificantOwner(e):                     # 0x1047E9E0
    cur = e
    while cur.isOK():
        if getBool(cur, 0x5657A0A): return cur    # 0x1047EA54 jnz
        cur = cur.owner()                          # 0x1047EA5D
    return cur                                     # 无效引用
```

三点与我方不同：**从元素自己开始判**；终止条件是 noun 位而不是"遇到 SITE/ZONE/WORL"；
**没有深度上限**，也没有 loop 容器特例——loop 容器本来就不 significant，自然被跨过。

- 我方：◐ `MAX_ANCESTOR_DEPTH = 32`，且 `collect_chain` 遇到粗层级容器就停。
  深度上限是**防御性**的（防 owner 环），不是语义——但它会让超深链静默截断。

### R15 删除的 primitive 不上卷 ✅（语义一致，实现不同）

`state == Deleted` 时 push 自己，不 push owner。

- 我方：删除集收敛到最顶端 + `delete_inst_relate_subtree`。形状不同，需实测比对。

### R16 负实体上卷是死代码 ⚪

`m_granularityMode`（`+0x20`）恒为 0：全类地址区间 `0x1047BC40`–`0x1047ED00` 里
`+20h` 只有**一处写**——构造函数 `0x1047BCCB` 的 `mov dword ptr [ebx+20h], 0`，
其余六处全是读；53 个方法无 setter；唯一构造点 `sub_10425CD0` 也只写 0。

连带：`IsNegative` 与 `Members(Negative)` 在本类内无活调用者；
字段 `0x92663` 无消费者，**P0 不必导 negative 表**。`ExistsCPS` 里
`cmp dword ptr [esi+20h], 1`（`0x1047D2FD`）那条同理不可达。

## 5. 去重：`IsPending`（R17–R20）

`0x1047DDD0`，在 `ModelToUpdate` 里作为最后一道门。**三个 state 三套判法，不是一套。**

### R20 去重键：非 primitive 元素按它的 SignificantOwner 判 ★新增 ❌

```
key = IsPrimitive(e) ? e : SignificantOwner(e)
```

三个 state 共用这一步。也就是说 core 的去重**先归一化到块**，再在块的层面比。

- 我方：❌ 我们的键是 `(action, target_refno)`，`target` 已经是解析后的生成根，
  形状上接近，但我们对 `Skip` / `TransformOnly` 类目标不做这个归一。

### R17 `state == Changed(0)`：沿 owner 链找 New 或 Changed，再看子孙 New，再看祖先删除标记 ★更正 ❌

```
cur = key
while cur.isOK():
    if queue has (cur, New):      return true     # 0x1047DE4x 内层扫描
    if queue has (cur, Changed):  return true
    cur = cur.owner()
if key.isOK() and IsDecendantPresent(key, New): return true    # 0x1047DFD4
if key.isOK() and IsPresent(key, AncestorDelete=2): return true
return false
```

`IsDecendantPresent(key, s)`（`0x1047DC30`）= 队列里存在一条 state 为 `s` 的记录，
它沿 owner 链能走到 `key`（即该记录是 `key` 的**子孙或它自己**）。

证据文档 §9 把三个 state 归纳成"沿 owner 链逐级上行"，只有这一个 state 是这样。

### R18 `state == New(1)`：沿 owner 链只找 New ❌

`key` 本身不 OK 就直接返回假，不做后面两步。

### R19 `state == Deleted(3)`：只看自身，**不上行** ★更正 ❌

```
return IsPresent(key, Deleted) or IsPresent(key, MemberOfChangedSignificant=4)
```

没有 owner 链遍历。证据文档的框架式描述在这里会误导实现者。

### 我方现状（R17–R20 共同）

`BTreeMap<(ModelWorkAction, String), ModelWorkItem>` —— **同一个 refno 才合并**。
祖先已经排了整根重生成、后代还各排一条，在我们这边是两条工作项。
**这是"多做"不是"漏做"**，所以计划把 P3.3 排在最后。

## 6. 收尾与执行（R21–R29）

### R21 `AncestorDeletes`：判据对了，终止条件写反了 ★更正 ❌

```
AncestorDeletes(e, state):                        # 0x1047C060
    if state not in {Deleted, MemberOfChangedSignificant}: return   # 0x1047C079/07E
    cur = e.owner()
    while cur.isOK():
        if getBool(cur, 0xA103E)                  # 0x1047C0E6 jnz → 标记
           or getBool(cur, 0xBBD5ADC)             # 0x1047C109 jnz → 标记
           or getBool(cur, 0x5657A0A):            # 0x1047C131 jz  → 跳过本级
            if not (queue has (cur, AncestorDelete)):   # 0x1047C140 cmp [esi+14h],2
                push(cur, AncestorDelete=2)             # 0x1047C192
        cur = cur.owner()                         # 0x1047C1A6 —— ★ 无条件继续
```

判据 = **`IsPrimitive(anc) ∨ IsSignificant(anc)`**（证据文档 §8 这条对）。

**但"命中已标记祖先时整条上行链终止"是反的。** 逐跳追下来：
`0x1047C14F jnz → 0x1047C1F4`（`mov al, 1`）→ `0x1047C1F6 jmp → 0x1047C15F`
（`test al, al`）→ `0x1047C161 jnz → 0x1047C197` —— `0x1047C197` 正好落在
push 调用（`0x1047C192`，5 字节）之后，**继续往下走到 `0x1047C19D` 取 owner 并循环**。

所以命中只是**跳过这一级的 push**，上行链照走到顶。**整条 owner 链上每一个
primitive-或-significant 的祖先都会被标记**，不是只标最近的那一个。

按证据文档原话实现，删除路径的祖先标记会少一大半，R24 的第二个消费者也就跟着失效。

- 我方：❌ 无对应物。计划 T3.3 引用的是错的那一版，已同步更正。

### R22 `AbsentPrimitives`：缺失图元回收 ❌

```
AbsentPrimitives(e, state):                       # 0x1047BE10
    if state in {Deleted, MemberOfChangedSignificant}: return   # 只在非删除类动作
    idlist = GetIDList()
    for p in Members(e, Primitive):               # mode 1：整棵子树的 primitive
        present = idlist.active and idlist.exists(p)
        if not present: push(p, AbsentPrimitive=5)
    idlist.release()
```

即：**重画一个块之前，把块内"模型里有、当前 ID 清单里没有"的图元行清掉。**
这是线上偶发"孤儿 mesh 行"该由谁负责的答案。

- 我方：**无**。计划 T3.1，最高优先。

### R23 ID 清单不活跃时，全部 primitive 判为缺失 ★新增 ⚠ 待确认

`idlist.active`（`PDMS_Idlist2 +0x18`）为假时 `present` 直接取 0——
于是**整棵子树的 primitive 全部入队 state 5**，在 pass 3 被逐个擦掉。
而 pass 2 刚刚把这个块画完。

`GetIDList`（`0x1047D650`）在视图没有绑定清单元素时返回一个"空清单"对象，
`+0x18` 应当就是这个"有没有绑定"的位。

**这条不下判词。** 它要么是 core 依赖"视图必有清单"这个前置、要么是一条有意的
"没清单就别留图元行"规则。移植 R22 之前必须在 live 进程上看一眼实际取值，
否则我们会照抄一条把刚画好的东西擦掉的逻辑。

### R24 `AncestorDelete(2)` 有**两个**消费者 ★更正 ❌

证据文档 §2 说 state 2"不处理——只作为 `IsPending`/`IsPresent` 的抢占标记"。
第一个消费者对，但漏了第二个：

1. **`IsPending(e, Changed)`** → `IsPresent(key, 2)`（R17 最后一步）。
   注意**只有 Changed 这一个 state 看它**，New 和 Deleted 都不看。
2. **`EraseModel(DB_Ref&)`（`0x1047C6D0`）** → 队列里**只要存在任意一条** state 2 记录，
   图元擦除就从 `ErasePrimitiveFromUnknownModel` 切到 `ErasePrimitiveFromCandidateModel`：

```
EraseModel(ref):
    if EraseSignificant(ref): return true
    if IsAncestorDeletesPresent():                      # 内联的 0x1047DC00
        return ErasePrimitiveFromCandidateModel(ref)    # 0x1047CB80
    else:
        return ErasePrimitiveFromUnknownModel(ref)      # 0x1047CCF0
```

这是**全局条件**，不是按元素判——一次 `Update` 里只要有任何祖先删除标记，
所有走 `DB_Ref` 重载的擦除都换策略。

### R25 `EraseModel` 是两个函数，按 significant 位分派 ★新增 ⚪

| 重载 | 地址 | 分派 | 谁调用 |
|---|---|---|---|
| `EraseModel(DB_Element&)` | `0x1047C670` | `getBool(e, 0x5657A0A)` 真 → `EraseSignificant(e.ref())`，假 → `ErasePrimitive(e)` | pass 1 的 state 0/1；pass 3 的 state 5 |
| `EraseModel(DB_Ref&)` | `0x1047C6D0` | 见 R24 | pass 1 的 state 3/4 |

指令：`0x1047C683 push 5657A0Ah` → `0x1047C6A5 jz` → 两条路。

- 我方：⚪ 我们的模型行删除不分这两类。**但它给出一条设计约束**：
  "块的行"和"图元的行"在 core 里是两套存储、两条擦除路径。
  移植 R10（块内成员清理）与 R22（缺失图元回收）时，删的**不是同一种行**。

### R26 `Exists` 对 significant 元素递归全子树 ★新增 ⚪

```
Exists(e, idlist):                                # 0x1047D050
    if IsPrimitive(e):
        if ExistsCPS(e, mode=1):                       return true
        if idlist.active and idlist.exists(e):         return true
    if not getBool(e, 0x5657A0A):                      return false   # 0x1047D0E7
    for m in Members(e, Primitive):                                   # 0x1047D112
        if Exists(m, idlist):                          return true    # 0x1047D127 递归
    return false
```

伪码里这后半段整个不见了（`getField(v6); return 0;`）。语义："这个元素或它子树里的
任何图元，现在还有没有可画的东西"。

### R27 `Update` 三遍 + 一次收尾 ⚪

```
Update():                                          # 0x1047EAB0
    if !m_enabled or !m_view.isOK() or queue.empty() or m_inDraw or m_muted: return
    idlist = GetIDList()

    pass 1: for rec in queue:                      # 就地遍历，可删元素
        if rec.state <= 1:                         # Changed / New
            if !EraseModel(elem(rec)) and !Exists(elem(rec), idlist):
                从队列中就地删除该条，不前进游标        # R28
        elif rec.state in {3, 4}:
            EraseModel(ref(rec))                   # ★ 另一个重载，见 R25

    pass 2: for rec in queue: if rec.state <= 1: m_inDraw = 1; DrawModel(rec)
    pass 3: for rec in queue: if rec.state == 5: EraseModel(elem(rec))
    Finish(); m_inDraw = 0
    idlist.release()
```

`DrawModel`（`0x1047C350`）不是函数调用，是发一条 PML：

```
DLL_PMLCommand::Run("PUPDES " + <view ref> + " MODEL " + <element ref> + " FORCE SUPPRESS")
```

- 我方：⚪ 我们跑自研生成管线，重画动作不可比、也不需要比。
  **但三遍的顺序是可比的**：先全擦、再全画、最后清缺失图元。

### R28 pass 1 的"擦不动且不存在 → 从队列删除" ⚪→❌

`!EraseModel(rec) && !Exists(rec, idlist)` 同时成立时，**该条被就地移出队列**，
于是 pass 2 不会画它。意思是：**不画本来就不在视图里的东西。**

- 我方：❌ 概念上对应"重生成前先确认目标还在范围内"。我们没有这一步——
  目标在窗口内被删掉时，`RegenRoot` 仍会执行一次空生成。不是正确性问题，是白跑。

### R29 `Finish` 清空队列 ⚪

```
Finish():                                          # 0x1047D380
    hasDeleted = queue 里存在 state == 3 的记录
    m_queue.end = m_queue.begin                    # 清空
    if hasDeleted: <视图侧的一次额外通知>
    <从 Resolver 取服务，回调 vtable[1](m_view)>
```

队列在 `Finish` 里清空，不在 pass 3 之后逐条清。配合 R6（`Refresh` 也清空），
队列的生命周期是"一次 `Update` 或一次 `Refresh`"。

## 7. 本轮对证据文档的更正

| 位置 | 原文 | 实际 | 影响 |
|---|---|---|---|
| §2 | state 2「不处理」 | 有第二个消费者：切换图元擦除策略（R24） | 移植删除路径时会漏掉一整条分支 |
| §3 | 门顺序 enabled→view→muted→XGEOM→DESI，五道 | 实际六道且顺序不同，多一道 `isValid`（R3/R4） | 排查线上问题时对不上 |
| §8 | 「命中已标记祖先时**整条上行链终止**」 | 只跳过该级 push，**上行照走到顶**（R21） | **计划 P3.3 会实现错**，祖先标记少一大半 |
| §9 | 三个 state 都「沿 owner 链上行」 | 只有 Changed 上行；Deleted 只看自身（R17/R19） | 删除去重会做成过度合并 |
| §1 | 只列 `NewModelToUpdate` | 还有 `NewModelNotify`，差一个落库副作用（R7） | — |

新增六条证据文档没有的规则：R3、R6、R7、R8、R20、R23，以及 R25/R26/R28 三条实现约束。

## 8. 仍未钉死的

- `IsPrimitive` 两个位各自的**字典名**未知——只知道 `0xA103E` 跨版本稳定。
- R23 的 `idlist.active == false` 语义，必须在 live 进程上看。
- `ExistsCPS` / `WWSegment` / `PDMS_Idlist2` 的内部结构——core 自己的绘制存储，
  移植时只需知道它们是"已绘制段"和"应绘制清单"两个集合，内部布局不必抄。
- §4 末尾登记的同族字段（`847458`、`606263`、`602413`、`595979`、`599813`）语义未追。
- 2.10 `sub_102C25F0` 里 `*(this+9) == 779672` 那条早退——它属于另一个类，
  但同样在读 significant 位，可能是一条例外规则。
- `PostSetRefListAttribute`（core.dll `0x5EAA7A3`）的 back-ref 表结构——ADR-003 那条线。

## 9. 缺口汇总（按"缺了会不会导致模型错"排序）

| 级别 | 规则 | 缺口 | 计划项 |
|---|---|---|---|
| **错** | R22 | 缺失图元回收——孤儿 mesh 行只能等整库重建 | T3.1 |
| **错** | R10 | 块内成员清理——同一几何可能出现两份 | T3.2 |
| **错** | R2 | XGEOMETRY 门——显式几何被卷进增量 | T2.2 |
| **错** | R6 | `Refresh` 不清队列——重建后跑无用增量，可能覆盖新结果 | **计划里没有，需新增** |
| 慢 | R17–R20 | 祖先抢占去重——多做不是漏做 | T3.3（含 R21 更正） |
| 慢 | R28 | 目标已消失仍跑一次空生成 | 未立项 |
| 判据 | R9/R12/R14 | 名单 vs 位表 | P0 / P1 / T2.1 |
| 已实现未启用 | — | CATA 反向级联（`UpdateScope::admits` 不放行） | P5 |
