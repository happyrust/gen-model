# 逆向取证：Core3D `PartialUpdateDesiMgr` —— 模型增量更新影响判定的完整实现

> 日期：2026-08-27
> 对象：`D:\AVEVA\Everything3D3.1\Core3D.dll`（32 位，ida-bridge 实例 `idalib-32268`）
> 旁证：`D:\AVEVA\Everything3D3.1\core.dll`（`idalib-48392`）、`Everything3D2.10\Core3D.dll`（`idalib-32872`）
> 关联：ADR-002（core.dll 权威范围）、ADR-003（反向级联索引）、ADR-009（DB_UserChanges 六桶）、
> ADR-032（元素 diff 的两条已知边界）；`src/data_interface/model_impact.rs`、
> `src/data_interface/generation_root.rs`、`src/data_interface/model_update_plan.rs`
> 原始转储：`.ida_scratch/analysis/partial_update_desimgr.c`、`partial_update_fields.txt`、
> `partial_update_rest.txt`、`partial_update_drivers.c`；探针：`.ida_scratch/probes/partial_update_*.py`

## 0. 这一轮补上了什么

此前的逆向线（`plant-model-gen/docs/reverse/*`、ADR-002/003/032）已经钉住了**变化采集**这一半：
`DB_DB::elementsChangedBetween` → `DB_UserChanges` 六桶 → `DCHC/EVALAT` 决定进不进 `QCHGLS`。
但**「拿到变化之后，究竟按什么粒度重画什么」**一直只有一个名字（`GranularityExpansion`）和
一句转述（"IsPrimitive→SignificantOwner→Members"），没有代码。

本轮把这一半整个拿了下来：`PartialUpdateDesiMgr` 这个类的 53 个方法全部定位，关键 14 个反编译，
粒度判据的三个 **noun 描述符字段 id** 从指令流里取出。结论是**它比我们以为的简单，也比我们以为的严格**：

- 没有"按属性分类决定重算范围"这回事——**改了什么属性在这一层已经不可见**；
- 粒度完全由 **noun 上的两个 bool 位**决定，不是由类型名单决定；
- 有三条我们完全没有的规则：XGEOMETRY 门、祖先抢占去重（`IsPending`）、缺失图元回收（`AbsentPrimitives`）。

第二轮补查（同日）关掉了 §14 上的三个待办，其中一个改变了结论：**负实体那条粒度分支是死代码**
（`m_granularityMode` 恒为 0，§6.1）。所以判据实际只有两个 bool 位，
`IsNegative` 的那个 int 位在这条链上没有活的消费者。同时补齐了 `AncestorDeletes` 的祖先判据（§8），
并把跨版本结论修正为"字段 id 稳定、函数不稳定，且第二个 primitive 位跨版本会换"（§4）。

> **第三轮（2026-08-28）：把主链上此前只有名字的函数也从指令流取下来**
> （`ModelToUpdate`、`IsPending`、`IsPresent`、`IsDecendantPresent`、`Members`、
> `Exists`、`Update`、`EraseModel` 两个重载、`Finish`、`Refresh`、`NewModelToUpdate`）。
> 中心算法 `GranularityExpansion`（§6）逐跳复核**无误**，但这一轮改了本文四处，
> 其中 §8 那处会导致照抄实现出错：
>
> | 位置 | 原文 | 实际 |
> |---|---|---|
> | §2 | state 2「不处理」 | 有第二个消费者：切换图元擦除策略 |
> | §3 | 五道门，muted 在 XGEOM 之前 | 六道门，多一道 `isValid`，顺序不同 |
> | §8 | 命中已标记祖先「整条上行终止」 | **只跳过该级 push，上行照走到顶** |
> | §9 | 三个 state 都「沿 owner 链上行」 | 只有 Changed 上行，Deleted 只看自身 |
>
> 逐条可核对的规则表（含我方现状与缺口排序）已单独成文，不再往本文里堆：
> `docs/specs/core3d-partial-update-conformance.md`，配套用例集
> `docs/specs/core3d-partial-update-test-cases.md`。本文继续作为**叙述性取证记录**。

## 1. 类的全貌与地址

`PartialUpdateDesiMgr` 是单例（`Instance()` @ `0x104194E0`），三个公共入口分别由三个自由函数驱动，
驱动函数从 `Resolver::instance()` 取到单例后直接调用：

| 公共入口 | 地址 | 驱动 | 传入的 `ModelState` |
|---|---|---|---|
| `ChangedModelToUpdate()` | `0x1047C200` | `sub_1041FA10` | `0`（Changed） |
| `NewModelToUpdate(elem)` | `0x1047E6E0` | `sub_1041FC30` | `1`（New） |
| `NewModelNotify(elem)` | `0x1047E670` | — | `1`（New） |
| `DeletedModelToUpdate(elem)` | `0x1047C2E0` | `sub_1041FB10` | `3`（Deleted） |
| `Update()` | `0x1047EAB0` | `sub_104203B0` | —（消费队列） |
| `Refresh(elem)` | `0x1047E800` | `sub_10420290` | — |

**`New` 有两个入口，差一个落库副作用。** `NewModelToUpdate` 在 `IsPending` 放行后
会取视图 ID 清单，若清单活跃且不含该元素，就把 `SignificantOwner(elem)` 并进清单并
`PDMS_Idlist2::writeDB()`（`AddIDList` `0x1047BF80`）；`NewModelNotify` 走同一个
state 1 但**没有**这一段。这一步决定了 `AbsentPrimitives`（§8）判"缺失"时看到的是什么。

**`Refresh` 不是"刷新一个元素"，是清空整个队列。** 只在传入元素是 `NOUN_VIEW`
且等于当前视图时动作：`m_queue.end = m_queue.begin`，所有排着的待办作废——
视图自己要整个重画了，增量没有意义。

内部主链：`ModelToUpdate` `0x1047E590` → `GranularityExpansion` `0x1047D8C0` →
`{SignificantOwner 0x1047E9E0, Members 0x1047E240, AncestorDeletes 0x1047C060, AbsentPrimitives 0x1047BE10}`
→ 队列 push `sub_1047BA50` → `Update` → `{EraseModel 0x1047C670, DrawModel 0x1047C350}`。

`IsPending` `0x1047DDD0`、`IsPresent` `0x1047E020`、`IsDecendantPresent` `0x1047DC30` 是去重判据。

## 2. `ModelState`：六个值，其中三个是内部态

反编译里 `ModelState` 是裸 int。从三个入口和内部 push 点反推，取值与含义：

| 值 | 来源 | 在 `Update()` 里的处理 |
|---|---|---|
| 0 Changed | `ChangedModelToUpdate` | 先 `EraseModel`，再 `DrawModel` |
| 1 New | `NewModelToUpdate` | 先 `EraseModel`，再 `DrawModel` |
| 2 AncestorDelete | `AncestorDeletes` 内部 push | 三遍都不消费它，但它有**两个**间接消费者（见下） |
| 3 Deleted | `DeletedModelToUpdate` | `EraseModel`，不重画 |
| 4 MemberOfChangedSignificant | `GranularityExpansion` 内部 push | `EraseModel`，不重画 |
| 5 AbsentPrimitive | `AbsentPrimitives` 内部 push | 第三遍单独 `EraseModel` |

队列元素是 24 字节：`DB_Ref`(12) + 两个字(元素句柄的 3/4 号字) + `state`(4)，push 走
`sub_1047BA50`（就是 `std::vector<Record>::push_back`，`add dword ptr [edi+4], 18h` 是 stride 证据）。

**state 2 的两个消费者**（第三轮更正——原文说它「不处理」，只说对了一半）：

1. `IsPending(e, Changed)` 的最后一步 `IsPresent(key, 2)`。
   **只有 `Changed` 这一个 state 看它**，`New` 和 `Deleted` 都不看（§9）。
2. `EraseModel(DB_Ref&)` `0x1047C6D0`——队列里**只要存在任意一条** state 2 记录，
   图元擦除就从 `ErasePrimitiveFromUnknownModel`（`0x1047CCF0`）切到
   `ErasePrimitiveFromCandidateModel`（`0x1047CB80`）：

   ```
   EraseModel(ref):
       if EraseSignificant(ref): return true
       if IsAncestorDeletesPresent():                    # 内联的 0x1047DC00
           return ErasePrimitiveFromCandidateModel(ref)
       return ErasePrimitiveFromUnknownModel(ref)
   ```

   这是**全局条件**，不是按元素判：一次 `Update` 里只要有任何祖先删除标记，
   所有走 `DB_Ref` 重载的擦除都换策略。

## 3. 通用门（六道，我们只实现了其中一道）

内部总入口 `ModelToUpdate` `0x1047E590` 的门是六道，**第三轮从伪码逐条核对后更正如下**
（原文写成五道、且把 `muted` 排在 XGEOM 之前）：

```
if (!m_enabled)                                   return;   // this+0x1C
if (!DB_Element::isOK(m_view))                    return;   // this+0x08，当前视图
if (!DB_Element::isValid(elem))                   return;   // ★ 原文漏了这一道
if (DB_DB::type(elem->getDB()) != 1)              return;   // 只处理 DESI 库
if (!DB_Element::climb(elem, NOUN_XGEOM).isNull())return;   // ★ XGEOMETRY 子树整体排除
if (m_muted)                                      return;   // this+0x1D，Mute()/UnMute()
if (IsPending(elem, state))                       return;   // 去重，见 §9
GranularityExpansion(elem, state);
```

语义上顺序无差别（全是无副作用的早退），但排查线上问题时要按这个顺序对。

三个公共入口（`ChangedModelToUpdate` / `NewModelToUpdate` / `DeletedModelToUpdate`）
在调 `ModelToUpdate` 之前**自己先查一遍** `m_enabled` / `m_view.isOK()` /
XGEOM / `m_muted`——XGEOM 门是唯一被重复检查的一道。`ChangedModelToUpdate` 例外：
它不查 XGEOM，因为它的每一条都来自 QCHGLS，逐条转交 `ModelToUpdate` 时才过门。

- **`DB_DB::type == 1`（只处理 DESI）** —— 我们有等价物（`UpdateScope::admits`）。
- **XGEOMETRY 门** —— 我们**没有**。`NOUN_XGEOM` 是全局 `DB_Noun const * const NOUN_XGEOM`
  （`0x1047DD5E` 处取），对应字典里的 `XGEOMETRY`/短名 `XGEOM`，hash `7739277`
  （见 `output/noun_layout.json:11666`）。凡是有 XGEOMETRY 祖先的元素，core 一律**不进**局部更新——
  这类显式几何走另一条路。`IsManagedStructure()` `0x1047DD50` 就是这个判断本身。

## 4. 粒度判据 = noun 描述符上的三个字段

这是本轮最关键的一条。`IsPrimitive` / `IsSignificant` / `IsNegative` 里**没有任何类型名单**，
全部是 `DB_Noun::getField(fieldId, out)`——和我们已经在用的
`db_get_element_info(noun_hash, 297853135)`（`primaryList`）是同一套机制、同一个函数族
（core.dll 里 `?getField@DB_Noun@@QBE_NHAA_N@Z` / `...HAAH@Z`）。

| 判据 | 实现（地址） | 字段 id（hex / dec） | 类型 |
|---|---|---|---|
| `IsSignificant(e)` | `0x1047E0D0` | `0x5657A0A` / `90536458` | bool |
| `IsPrimitive(e)` | `0x1047E070` | `0xA103E` / `659518`，**为假时再试** `0xBBD5ADC` / `196958940` | bool ∨ bool |
| `IsNegative(e)` | `0x1047DD90` | `0x92663` / `599651` | int ≠ 0 |

指令证据（`.ida_scratch/analysis/partial_update_fields.txt`）：

```
1047E07A  push    0A103Eh            ; IsPrimitive 第一位
1047E0AB  push    0BBD5ADCh          ; IsPrimitive 第二位（第一位为假才读）
1047E0DA  push    5657A0Ah           ; IsSignificant
1047DD99  push    92663h             ; IsNegative（int 版 getField）
```

**取值走出参，不走返回值**（三条指令流一致，`0x1047DD90` / `0x1047E070` / `0x1047E0D0`）：
每个判据都先把栈上的 out 变量清零，再 `getField(id, &out)`，最后**读 out**——
`getField` 自己的 bool 返回值（"这个 noun 有没有登记这个字段"）被丢掉。
`IsPrimitive` 的两次调用共用同一个 out 变量，且只在第一次前清零；
因为只有第一次读出假才会走到第二次，复用是安全的。

> **这一条直接约束导出器**：`dump_core_primary_list.py` 那条 frida 通道必须同样取出参、
> 并在调用前清零，否则"字段未登记"的 noun 会读到上一次的残留值。
> 按 core 的口径，字段未登记 = 该位为假。

`IsNegative` 是 `int` 版 `getField`（`?getField@DB_Noun@@QBE_NHAAH@Z`）后接 `setnz`，
即**非零为真**，不是 `== 1`。（2.10 的 `MassProperties::PopulateCSGtree` 在同一个 id 上用的是
`== 1`，见下——同一字段在不同调用点的判法不同，导出时应存原始 int 而不是 bool。）

**版本稳定性：id 稳定，函数不稳定。** 四个 id 在 E3D 2.10 的 `Core3D.dll` 里都能按字节搜到，
但**没有一个落在 `PartialUpdateDesiMgr` 的对应函数里**：

| id | 2.10 命中 | 所在函数 |
|---|---|---|
| `0x5657A0A` significant | `0x102C260D` | `sub_102C25F0` |
| `0xBBD5ADC` primitive-B | `0x102C2631` | `sub_102C2620` |
| `0xA103E` primitive-A | 6 处，最近的 `0x10425EF0` | 均与本类无关 |
| `0x92663` negative | 3 处 | 含 `sub_106FDA80` |

`sub_102C25F0` / `sub_102C2620` 是**另一个类**的两个相邻方法：它们把 `DB_Element` 缓存在
对象 `+0x10`（`PartialUpdateDesiMgr` 的元素在 `+8` 且判据是传参），而且 `sub_102C25F0` 多一条
早退 `if (*(this+9) == 779672) return false`。所以 2.10 证明的是**字段词汇表跨版本没换**，
不是"同一个函数还在"。对 P0 取数来说这够用了。

**2.10 里的第二个 primitive 位不是同一个。** `sub_106FDA80`（`MassProperties::PopulateCSGtree`）
判 primitive 用的是 `getBool(659518) || getBool(661624)`，判 negative 用的是
`getInt(599651) == 1 || getBool(661624)`。而 **`661624`（`0xA18B8`）在 3.1 的 `Core3D.dll` 里
一次都搜不到**。也就是说：`0xA103E` 是稳定的那一位，**它的搭档不稳定**——
2.10 的 `MassProperties` 配 `0xA18B8`，3.1 的 `PartialUpdateDesiMgr` 配 `0xBBD5ADC`。
快照必须记录来源版本，不能把"primitive"当成一个跨版本的固定谓词。

同一个函数里还露出一批同族的 noun 描述符字段，本轮未追语义，登记备查：
`847458`（`getInt == 5`）、`606263`、`602413`、`595979`、`599813`。

> 这两个字段就是我们缺的那份数据。`scripts/e3d/dump_core_primary_list.py` 现成的 frida 通道
> （`db_get_element_info(noun_hash, field_id)`）换个 `FIELD_ID` 就能把表导出来，
> 与 `tests/fixtures/core-primary-list-e3d31.json` 同规格入库。
> `negative` 那张表**不用导**——理由见 §6。

## 5. `SignificantOwner`：**含自身**的向上攀爬

```
DB_Ref SignificantOwner(const DB_Element& e):
    cur = e
    while (cur.isOK() && !getField(cur.actualType(), 0x5657A0A))
        cur = cur.owner()
    return cur                     // 攀到不 OK 就返回那个无效引用
```

三点与我们的 `resolve_element_generation_root` 不同：

1. **从元素自己开始判**——元素自身 significant 时就是它自己，不上溯；
2. 终止条件是 **noun 位**，不是"遇到 SITE/ZONE/WORL 就停"；
3. **没有深度上限**，也没有 loop 容器特例——loop 容器本来就不 significant，自然被跨过。

## 6. `GranularityExpansion`：完整算法

直接从 `0x1047D8C0` 的指令流还原（反编译器在这里把两条分支的顺序渲染反了，以指令为准）：

```
GranularityExpansion(e, state):
    if IsSignificant(e):                       # 0x5657A0A
        push(e, state)
        AncestorDeletes(e, state)
        AbsentPrimitives(e, state)
        for m in Members(e, SearchMode::Significant):     # 见 §7
            push(m, state == Deleted ? 3 : 4)             # 两者在 Update 里都只 Erase
            AncestorDeletes(m, state)
            AbsentPrimitives(m, state)
        return

    if !IsPrimitive(e):                        # 既不 significant 又不 primitive
        return                                 # ★ 什么都不排——core 直接丢弃

    if m_granularityMode == 0:                 # this+0x20
        if state == Deleted:
            push(e, Deleted)
        else:
            owner = SignificantOwner(e)
            push(owner, state)
            AbsentPrimitives(owner, state)
    else:
        target = Members(e, SearchMode::Negative) ? SignificantOwner(e) : e
        push(target, state)

    AncestorDeletes(e, state)
```

三条可以直接对照我们实现的结论：

- **significant 元素变了 → 整块重画，块内 significant 后代的模型行被逐个抹掉**（state 4）。
  这是"块级颗粒"的确切含义：不是"重画子树里每一个"，是"重画块，顺手删掉块内那些自己也曾单独成块的行"。
- **既不 significant 又不 primitive 的元素变了 → core 什么都不做。**
  我们的 `Unknown → Regen` 保守兜底在这里没有对应物；core 靠 noun 位把它挡在门外。
- **primitive 元素变了 → 上卷到 SignificantOwner 重画**，并对该 owner 做缺失图元回收。

### 6.1 `m_granularityMode ≠ 0` 那条分支是死代码

`m_granularityMode` 是对象 `+0x20` 的一个 int。**它在 E3D 3.1 里恒为 0**：

- 把整个类的地址区间（`0x1047BC40`–`0x1047ED00`）扫一遍，`+20h` 只有**一处写**——
  构造函数 `0x1047BCCB` 的 `mov dword ptr [ebx+20h], 0`；反编译的构造函数同样只有
  `*((_DWORD *)this + 8) = 0`。其余六处全是 `cmp`（读）。
- 53 个方法里**没有任何 setter**：公开可变的只有 `UpdateOn` / `Mute` / `UnMute` /
  `SetView` / `ResetView` / `SetViewForced`，都不碰 `+0x20`。
- 唯一构造该对象的函数是 `sub_10425CD0`（`0x10425EC9` 处 `call` 构造函数）。
  扫它的全部指令，对任何 `+20h` 的写入也只有 `mov dword ptr [edi+20h], 0`，没有非零写入。

所以 `GranularityExpansion` 的 `else` 分支——那条"只有带负实体成员的 primitive 才上卷"的
改判逻辑——**在生产里一次都不会跑**。连带地：

- `IsNegative`（`0x1047DD90`）与 `Members(SearchMode::Negative)` 在本类内**没有活的调用者**；
- 字段 `0x92663` 在这条链上没有消费者，**P0 不需要导出 negative 表**；
- 活的粒度规则就只有一句：
  `state == Deleted ? push(e, Deleted) : push(SignificantOwner(e), state) + AbsentPrimitives(owner, state)`。

`ExistsCPS`（`0x1047D2FD` 处 `cmp dword ptr [esi+20h], 1`）里那条 mode==1 分支同理不可达。

## 7. `Members` 的三个 `SearchMode`

`Members` `0x1047E240` 是一个显式栈的遍历：`DB_Element::members()` 取直接成员，逐个弹出，
对每个成员读同样的三个字段（`0x5657A0A` / `0xA103E`∨`0xBBD5ADC` / `0x92663`），然后按 mode 决定
**是否收集**与**是否继续下潜**：

| mode | 命名 thunk | 收集 | 下潜 | 指令区间 |
|---|---|---|---|---|
| 0 | `SignificantMembers` `0x1047E9C0` | significant 成员 | **只穿过 significant 成员** | `0x1047E37E`–`0x1047E3ED` |
| 1 | `PrimitiveMembers` `0x1047E7E0` | primitive 成员 | 所有成员，全子树 | `0x1047E3F9`–`0x1047E46C` |
| 2 | `NegativeMembers` `0x1047E650` | primitive **且** negative 的成员 | 只穿过 primitive **且非 negative** 的成员 | `0x1047E475`–`0x1047E4EC` |

**mode 0 的下潜规则有一条可判真假的推论**：非 significant 的子节点会**挡住**它下面的
significant 孙节点——后者不会被收集。这不是遍历实现的副作用，是判据本身
（`0x1047E37E` 的 `cmp` / `0x1047E381` 的 `jz` 一起跳过收集与下潜两件事）。
实现"块内成员清理"时若写成"全子树找 significant"，就会多删一批行。

mode 2 收集到一个 negative primitive 后**不再往下潜**（`0x1047E48C` 直接跳下一轮），
第三轮才看清；它挂在死代码上（§6.1），登记备查。

返回值是 bool：`out` 非空即真。`GranularityExpansion` 的 `m_granularityMode ≠ 0` 分支正是靠
mode 2 的返回值判断"这个 primitive 参与布尔运算吗"。

## 8. `AncestorDeletes` / `AbsentPrimitives`：两条我们没有的收尾

**`AncestorDeletes(e, state)` `0x1047C060`** —— 仅在 `state ∈ {3, 4}`（删除类）时动作。
反编译在这里丢了祖先判据（`HIBYTE(v17)` 是残留），从指令流补齐后是同样那三个字段：

```
AncestorDeletes(e, state):
    if state not in {3, 4}: return                      # 1047C079 / 1047C07E
    cur = e.owner()
    while cur.isOK():
        if getBool(cur, 0xA103E)                        # 1047C0E6  jnz  -> 1047C133
           or getBool(cur, 0xBBD5ADC)                   # 1047C109  jnz  -> 1047C133
           or getBool(cur, 0x5657A0A):                  # 1047C131  jz   -> 1047C19D（跳过本级）
            if not (queue contains (cur, state == 2)):  # 1047C140 cmp [esi+14h],2
                push(cur, 2)                            # 1047C192  call sub_1047BA50
        cur = cur.owner()                               # 1047C1A6 —— 无条件继续
```

判据就是 **`IsPrimitive(anc) ∨ IsSignificant(anc)`**——三个字段任一为真即标记，
一个都不真就跳过这一级继续往上。

> **更正（第三轮）。** 原文这里写的是「命中『祖先已被标记』时整条上行链终止」，**方向反了**。
> 逐跳追下来：`1047C14F jnz -> 1047C1F4`（`mov al, 1`）→ `1047C1F6 jmp -> 1047C15F`
> （`test al, al`）→ `1047C161 jnz -> 1047C197`。而 `1047C197` 正好落在 push 调用
> （`1047C192`，5 字节）**之后**，控制流继续走到 `1047C19D` 取 owner 并回到循环头
> `1047C0C0`。所以命中只是**跳过这一级的 push**，上行链照走到顶——
> **整条 owner 链上每一个 primitive-或-significant 的祖先都会被标记**，
> 不是只标最近的那一个。
>
> 按原文实现，删除路径的祖先标记会少一大半，上面 §2 那第二个消费者
> （擦除策略切换）也会跟着大面积失效。

如 §2 所述 state 2 在 `Update` 里不被消费，它的唯一作用是让后续 `IsPending`/`IsPresent`
认出"这一支已经因为删除被处理过了"。

**`AbsentPrimitives(sig, state)` `0x1047BE10`** —— 仅在 `state ∉ {3, 4}`（非删除）时动作：

```
idlist = GetIDList()
for p in Members(sig, SearchMode::Primitive):        # 整棵子树的 primitive
    if !(idlist.active && PDMS_Idlist2::exists(idlist, p)):
        push(p, 5)                                   # Update 第三遍 EraseModel
```

即：**重画一个块之前，把块内"模型里有、当前元素清单里没有"的图元行清掉**。
这正是我们线上偶尔出现的"孤儿 mesh 行"该由谁负责的答案。

> **一条不下判词的边界。** `idlist.active`（`PDMS_Idlist2 +0x18`）为假时，
> `present` 直接取 0——于是**整棵子树的 primitive 全部入队 state 5**，在 pass 3 被逐个擦掉，
> 而 pass 2 刚把这个块画完。`GetIDList`（`0x1047D650`）在视图没有绑定清单元素时
> 返回的正是这种"空清单"对象。要么 core 依赖"视图必有清单"这个前置
> （`NewModelToUpdate` 的 `AddIDList` 副作用在维持它，§1），要么这是一条有意的规则。
> **移植之前必须在 live 进程上看一眼实际取值**，否则会照抄一条把刚画好的东西擦掉的逻辑。

## 9. `IsPending`：祖先抢占去重

`IsPending(e, state)` `0x1047DDD0` 在 `ModelToUpdate` 和 `NewModelToUpdate` 里做前置拦截。
命中即返回真 → 本次变化**整个丢弃**。

第一步三个 state 共用——**先把判据归一化到"块"**：

```
key = IsPrimitive(e) ? e : SignificantOwner(e)
```

> **更正（第三轮）。** 原文接着写「然后沿 owner 链逐级上行」，把三个 state 归纳成一套。
> 实际是**三套**，只有 `Changed` 走 owner 链：

```
state == 0 (Changed):
    cur = key
    while cur.isOK():
        if queue has (cur, 1): return true        # 先扫 New
        if queue has (cur, 0): return true        # 再扫 Changed
        cur = cur.owner()
    if key.isOK() and IsDecendantPresent(key, 1): return true    # 队列里有 key 的子孙排着 New
    if key.isOK() and IsPresent(key, 2):          return true    # key 已被祖先删除标记打过
    return false

state == 1 (New):
    if !key.isOK(): return false
    cur = key
    while cur.isOK():
        if queue has (cur, 1): return true
        cur = cur.owner()
    return false                                   # 不看 2，也不看子孙

state == 3 (Deleted):
    return IsPresent(key, 3) or IsPresent(key, 4)  # ★ 完全不上行

其它 state: return false
```

`IsDecendantPresent(e, s)` `0x1047DC30` 的确切语义：队列里存在一条 state 为 `s` 的记录，
沿它自己的 owner 链能走到 `e`——即**该记录是 `e` 的子孙或就是 `e` 自己**。
`IsPresent(e, s)` `0x1047E020` 就是队列线性扫描 `rec.state == s && rec == e`。

我们今天的去重是 `BTreeMap<(action, target_refno)>` ——**同一个 refno** 才合并。
祖先已经排了整根重生成、后代还各排一条，在我们这边是两条工作项。

## 10. `Update()`：三遍 + 一次收尾

```
Update():                                            # 0x1047EAB0
    if !m_enabled or !m_view.isOK() or queue.empty()
       or m_inDraw or m_muted:      return           # m_inDraw = this+0x05，重入保护
    idlist = GetIDList()

    pass 1: for rec in queue:
                if rec.state <= 1:                   # Changed / New
                    if !EraseModel(elem(rec)) && !Exists(elem(rec), idlist):
                        队列里就地删除该条（尾部左移 24 字节），游标不前进
                elif rec.state in {3, 4}:
                    EraseModel(ref(rec))             # ★ 另一个重载，见 §2
    pass 2: for rec in queue: if rec.state <= 1: m_inDraw = 1; DrawModel(rec)
    pass 3: for rec in queue: if rec.state == 5: EraseModel(elem(rec))
    Finish(); m_inDraw = 0
    idlist.release()
```

**`EraseModel` 是两个不同的函数，pass 1 的两条分支各用一个**：

| 重载 | 地址 | 分派 | 用在哪 |
|---|---|---|---|
| `EraseModel(DB_Element&)` | `0x1047C670` | `getBool(e, 0x5657A0A)` 真 → `EraseSignificant`，假 → `ErasePrimitive`（`0x1047C683` push / `0x1047C6A5` jz） | pass 1 的 state 0/1、pass 3 的 state 5 |
| `EraseModel(DB_Ref&)` | `0x1047C6D0` | 见 §2 的 state-2 第二消费者 | pass 1 的 state 3/4 |

也就是说"块的行"和"图元的行"在 core 里是两套存储、两条擦除路径。
移植块内成员清理与缺失图元回收时，删的**不是同一种行**。

pass 1 那句"擦不动、清单里也没有 → 把条目删掉"是**不画不在视图里的东西**：
只有本来就有模型、或者在当前 ID 清单里的元素才会走到 pass 2。
`Exists`（`0x1047D050`）本身是递归的——对 significant 元素它会遍历整棵子树的 primitive
逐个再问一遍（伪码把这后半段整个丢了，`0x1047D112` 的 `Members(mode=1)` 与
`0x1047D127` 的自调用是证据）。

`Finish`（`0x1047D380`）先记下队列里有没有 state 3，然后**把队列整个清空**
（`m_queue.end = m_queue.begin`），再从 `Resolver` 取服务回调视图。
所以队列的生命周期是"一次 `Update` 或一次 `Refresh`"（§1）。

**`DrawModel` 不是函数调用，是发一条 PML 命令**（`0x1047C350`）：

```
DLL_PMLCommand::Run("PUPDES " + <view ref> + " MODEL " + <element ref> + " FORCE SUPPRESS")
```

## 11. `ChangedModelToUpdate` 丢掉了 DCHC code

```
list = <change/QCHGLS>                     ; sub_1022C3D7 → MTRENT("change/QCHGLS", 13, …)
n    = HQLNIR(list)
for i in 1 .. n step 3:                    ; ★ 步长 3
    HGETIA(list, i, buf, count=2)          ; ★ 只取 2 个字 = DB_Ref
    ModelToUpdate(DB_Element(DB_Ref(buf)), 0)
```

QCHGLS 每条 3 个字：`ref_hi, ref_lo, changeCode`。**图形局部更新只读前两个字，change code 被丢弃，
每一条都当 `Changed(0)` 处理。** 也就是说 DCHC 码的全部作用发生在上游——决定这个元素**进不进**
QCHGLS；一旦进了，重画范围只由 noun 位决定。

> 这条直接支持 ADR-002 的取舍：我们在 `OperationEffectSummary.max_dchc` 上保留原始码是可以的，
> 但**不该**让它去调节重生成范围——core 自己在这一层就不看它。同时它也说明
> `TransformOnly` 这条便宜路径在 core 里**根本不存在**：POS/ORI 改动进了 QCHGLS 就是一次整块重画。

## 12. 与我们实现的逐条对照

| 关注点 | Core3D `PartialUpdateDesiMgr` | gen-model 今天 | 判定 |
|---|---|---|---|
| 变化 → 状态 | Changed / New / Deleted 三入口 | `OperationImpact::{Regen, TransformOnly, Skip}` + 六桶 | ◐ 我们多一条 TransformOnly，core 没有 |
| 属性影响判定 | **这一层不存在**（上游 DCHC 决定进不进 QCHGLS） | `model_impact.rs` 五张表 + DCHC 快照 + A2 引用升级 | ✅ 分层等价，我们把上游那半自己实现了 |
| 库门 | `DB_DB::type == 1`（DESI） | `UpdateScope::admits` | ✅ |
| XGEOMETRY 门 | `climb(NOUN_XGEOM).isNull()` | **无** | ❌ 缺 |
| 粒度判据 | noun 位 `0x5657A0A` / `0xA103E`∨`0xBBD5ADC` / `0x92663` | 手写名单 `DEFAULT_DELIVERY_UNIT_TYPES` + `COARSE_HIERARCHY_NOUNS` + `is_loop_container_noun` | ❌ 近似，且数据源不同 |
| 上卷 | `SignificantOwner`（含自身，无深度上限） | `resolve_element_generation_root`（MDU 优先 → 显著 owner → 自身，深度 32） | ◐ 形状相同，判据不同 |
| 非 significant 非 primitive 的变化 | **丢弃** | `Unknown → Regen` 保守触发 | ◐ 我们更保守（多做），不是漏做 |
| primitive + 负实体 | 挂在 `m_granularityMode ≠ 0` 下，而该值恒为 0（§6.1） | 无负实体感知 | ⚪ core 里是死代码，不构成缺口 |
| 块内成员 | significant 后代逐个 `Erase`（state 4） | 无 | ❌ 缺（可能留孤儿行） |
| 缺失图元回收 | `AbsentPrimitives` → state 5 → `Erase` | 无 | ❌ 缺 |
| 祖先抢占去重 | `IsPending` 沿 owner 链查队列 | 仅 `(action, refno)` 精确去重 | ❌ 缺（多做，不是漏做） |
| 视图全量刷新 | `Refresh(VIEW)` 清空整个待办队列 | **无**——重建后仍会跑已作废的增量 | ❌ 缺（做无用功，且可能覆盖新结果） |
| 目标已消失 | `Update` pass 1 就地移出队列，不进 pass 2 | **无**——仍跑一次空生成 | ❌ 缺（白跑，不影响正确性） |
| 删除 | `Deleted` + `AncestorDeletes` 标记祖先 | 删除集收敛到最顶端 + `delete_inst_relate_subtree` | ◐ 形状不同，需实测比对 |
| 目录/规格反向级联 | `PostSetRefListAttribute` 维护 back-ref（core.dll，另一条线） | `cata_closure` + `ref_rev` + `CascadeExpand`，**但 `UpdateScope::admits` 不放行 CATA** | ❌ 已实现未启用 |
| 重画动作 | PML `PUPDES <view> MODEL <ref> FORCE SUPPRESS` | 自研生成管线 | ⚪ 不可比，也不需要比 |

## 13. 复现方法

```powershell
ida-bridge list                       # 目标：D:\AVEVA\Everything3D3.1\Core3D.dll → idalib-32268
ida-bridge exec idalib-32268 --sql "SELECT start_ea, name FROM funcs WHERE name LIKE '%PartialUpdateDesiMgr%' ORDER BY name"
ida-bridge exec idalib-32268 --timeout-s 300 -f .ida_scratch\probes\partial_update_dump.py     # → analysis\partial_update_desimgr.c
ida-bridge exec idalib-32268 --timeout-s 180 -f .ida_scratch\probes\partial_update_fields.py   # → analysis\partial_update_fields.txt
ida-bridge exec idalib-32268 --timeout-s 240 -f .ida_scratch\probes\partial_update_rest.py     # → analysis\partial_update_rest.txt
ida-bridge exec idalib-32268 --timeout-s 240 -f .ida_scratch\probes\partial_update_drivers.py  # → analysis\partial_update_drivers.c
ida-bridge exec idalib-32872 --sql "SELECT address FROM bin_search WHERE pattern = '68 0A 7A 65 05'"  # 2.10 交叉核对
```

第二轮（补查 §4 版本核对、§6.1 死分支、§8 祖先判据），纯 SQL，无需探针脚本：

```powershell
# 四个 id 在 2.10 的落点 + 所在函数
ida-bridge exec idalib-32872 --sql "SELECT '0xA103E' AS which, address FROM bin_search WHERE pattern = '68 3E 10 0A 00' UNION ALL SELECT '0xBBD5ADC', address FROM bin_search WHERE pattern = '68 DC 5A BD 0B' UNION ALL SELECT '0x92663', address FROM bin_search WHERE pattern = '68 63 26 09 00' UNION ALL SELECT '0x5657A0A', address FROM bin_search WHERE pattern = '68 0A 7A 65 05'"
ida-bridge exec idalib-32872 --sql "SELECT decompile(0x102c25f0) AS a, decompile(0x102c2620) AS b, decompile(0x106fda80) AS c"
# 0xA18B8 在 3.1 不存在（零命中即证）
ida-bridge exec idalib-32268 --sql "SELECT address FROM bin_search WHERE pattern = '68 B8 18 0A 00'"
# m_granularityMode：全类范围内对 +20h 的唯一写在构造函数
ida-bridge exec idalib-32268 --sql "SELECT address, disasm, name_at(func_start(address)) AS func FROM instructions WHERE address BETWEEN 0x1047bc40 AND 0x1047ed00 AND disasm LIKE '%+20h]%' ORDER BY address"
ida-bridge exec idalib-32268 --sql "SELECT decompile(0x1047bc40) AS ctor"
ida-bridge exec idalib-32268 --sql "SELECT from_ea, name_at(from_func) AS caller FROM xrefs WHERE to_ea = 0x1047bc40"          # 唯一构造点 sub_10425CD0
ida-bridge exec idalib-32268 --sql "SELECT address, disasm FROM instructions WHERE func_ea = 0x10425cd0 AND disasm LIKE '%+20h%'"
# 三个判据的精确取值语义（出参 vs 返回值）
ida-bridge exec idalib-32268 --sql "SELECT address, disasm FROM instructions WHERE func_ea = 0x1047dd90 ORDER BY address"
# AncestorDeletes 的祖先判据与分支去向
ida-bridge exec idalib-32268 --sql "SELECT address, disasm FROM instructions WHERE func_ea = 0x1047c060 AND address BETWEEN 0x1047c0b8 AND 0x1047c150 ORDER BY address"
```

第三轮（主链补全 + §2/§3/§8/§9 更正），同样纯 SQL：

```powershell
# 类的全部方法（修饰名在导出表里，换版本不用靠特征码）
ida-bridge exec idalib-32268 --sql "SELECT start_ea, end_ea - start_ea AS size, name FROM funcs WHERE start_ea BETWEEN 0x1047b000 AND 0x1047f000 ORDER BY start_ea"
# 主链：伪码够用的几个
ida-bridge exec idalib-32268 --sql "SELECT decompile(0x1047e590) AS ModelToUpdate, decompile(0x1047e020) AS IsPresent, decompile(0x1047dc30) AS IsDecendantPresent"
ida-bridge exec idalib-32268 --sql "SELECT decompile(0x1047ddd0) AS IsPending"
ida-bridge exec idalib-32268 --sql "SELECT decompile(0x1047c200) AS chg, decompile(0x1047e6e0) AS new, decompile(0x1047c2e0) AS del, decompile(0x1047e800) AS refresh, decompile(0x1047e670) AS notify"
ida-bridge exec idalib-32268 --sql "SELECT decompile(0x1047eab0) AS upd, decompile(0x1047d380) AS fin, decompile(0x1047c6d0) AS erase_ref"
# 伪码不够用的：只能读指令流
ida-bridge exec idalib-32268 --sql "SELECT address, disasm FROM instructions WHERE func_ea = 0x1047c670 ORDER BY address"                     # EraseModel 按 significant 位分派
ida-bridge exec idalib-32268 --sql "SELECT address, disasm FROM instructions WHERE func_ea = 0x1047e240 ORDER BY address"                     # Members 三模
ida-bridge exec idalib-32268 --sql "SELECT address, disasm FROM instructions WHERE func_ea = 0x1047d050 AND (mnemonic IN ('push','cmp','test','call') OR disasm LIKE 'j%')"   # Exists 的递归半边
ida-bridge exec idalib-32268 --sql "SELECT address, disasm FROM instructions WHERE func_ea = 0x1047d8c0 AND (mnemonic IN ('cmp','test','push') OR disasm LIKE 'j%' OR disasm LIKE 'call%')"  # GranularityExpansion 分支骨架
# §8 那处更正的关键跳转（1047C1F4 是 mov al,1 而不是 return）
ida-bridge exec idalib-32268 --sql "SELECT address, disasm FROM instructions WHERE address BETWEEN 0x1047c133 AND 0x1047c1fa ORDER BY address"
```

> **反编译器在这个类上系统性出错，凡涉及 noun 位判据的函数一律读指令流。**
> `getField(id, &out)` 的出参它认不出来，于是把调用渲染成无参、把 `out` 的比较整个丢掉。
> 后果不是"少个参数"，是**丢分支**：`GranularityExpansion` 的整条 significant 分支不见了，
> `Exists` 丢了递归的一半，`IsNegative` 整个反编译失败。
> 判定一个函数有没有被坑，看伪码里有没有孤零零的 `DB_Noun::getField(vN);` 后跟 `return 0`。

## 14. 明确没查的

### 本轮补查后已关闭

- ~~`m_granularityMode` 由谁设、默认值是什么~~ → §6.1：构造函数写 0，全类无 setter，
  唯一构造点也不写非零；`≠ 0` 分支是死代码。
- ~~`AncestorDeletes` 里筛选祖先的 noun 条件~~ → §8：`IsPrimitive ∨ IsSignificant`。
  （同轮给出的"命中已标记祖先时整条上行终止"是错的，第三轮已更正为**只跳过该级 push**。）
- ~~E3D 2.10 只核对了 `IsSignificant` 一个 id~~ → §4：四个 id 全部核对完毕。
  结论修正为"id 稳定、函数不稳定"，并发现 2.10 的第二个 primitive 位是 `0xA18B8`
  （3.1 中不存在），因此"primitive"不是跨版本固定谓词。

### 第三轮补查后已关闭

- ~~`ModelToUpdate` / `IsPending` / `IsPresent` / `IsDecendantPresent` / `Members` /
  `Exists` / `Update` / `EraseModel` / `Finish` / `Refresh` 只有名字没有算法~~
  → 全部取下，见 §1 / §3 / §7 / §9 / §10 与
  `docs/specs/core3d-partial-update-conformance.md` 的 R1–R29。
- ~~`GranularityExpansion` 的还原是否可靠~~ → 从 `0x1047D8C0` 逐跳复核**无误**。
  顺带确认 Hex-Rays 在这个类上不是"少渲染了参数"而是**丢分支**：
  significant 那整条（`0x1047D91A` 的 `jz`）在伪码里根本不存在，`Exists` 丢了递归的一半。

### 仍未查

- **`AbsentPrimitives` 在 ID 清单不活跃时会把整棵子树的 primitive 全判为缺失**
  （`0x1047BE10`：`idlist.active` 为假时 `present` 直接取 0），而 pass 2 刚把这个块画完。
  要么 core 依赖"视图必有清单"这个前置，要么是一条有意的规则——**必须在 live 进程上
  看一眼 `PDMS_Idlist2 +0x18` 的实际取值再决定移植时怎么处理**，否则会照抄一条
  把刚画好的东西擦掉的逻辑。
- `IsPrimitive` 两个位各自的字典名未知——只知道 `0xA103E` 是跨版本稳定的那一个。
- §4 末尾登记的同族字段（`847458` / `606263` / `602413` / `595979` / `599813`）语义未追。
- `sub_102C25F0`（2.10）里 `*(this+9) == 779672` 那条早退的含义——它属于另一个类，
  但同样在读 significant 位，可能是一条我们没看到的例外规则。
- `PostSetRefListAttribute`（core.dll `0x5EAA7A3` 处的符号）维护的 back-ref 表结构——
  这是 ADR-003 那条线，本轮没碰。
