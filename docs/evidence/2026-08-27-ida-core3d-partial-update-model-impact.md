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
- 粒度完全由 **noun 上的两个 bool 位 + 一个 int 位**决定，不是由类型名单决定；
- 有三条我们完全没有的规则：XGEOMETRY 门、祖先抢占去重（`IsPending`）、缺失图元回收（`AbsentPrimitives`）。

## 1. 类的全貌与地址

`PartialUpdateDesiMgr` 是单例（`Instance()` @ `0x104194E0`），三个公共入口分别由三个自由函数驱动，
驱动函数从 `Resolver::instance()` 取到单例后直接调用：

| 公共入口 | 地址 | 驱动 | 传入的 `ModelState` |
|---|---|---|---|
| `ChangedModelToUpdate()` | `0x1047C200` | `sub_1041FA10` | `0`（Changed） |
| `NewModelToUpdate(elem)` | `0x1047E6E0` | `sub_1041FC30` | `1`（New） |
| `DeletedModelToUpdate(elem)` | `0x1047C2E0` | `sub_1041FB10` | `3`（Deleted） |
| `Update()` | `0x1047EAB0` | `sub_104203B0` | —（消费队列） |
| `Refresh(elem)` | `0x1047E800` | `sub_10420290` | — |

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
| 2 AncestorDelete | `AncestorDeletes` 内部 push | **不处理**——只作为 `IsPending`/`IsPresent` 的抢占标记 |
| 3 Deleted | `DeletedModelToUpdate` | `EraseModel`，不重画 |
| 4 MemberOfChangedSignificant | `GranularityExpansion` 内部 push | `EraseModel`，不重画 |
| 5 AbsentPrimitive | `AbsentPrimitives` 内部 push | 第三遍单独 `EraseModel` |

队列元素是 24 字节：`DB_Ref`(12) + 两个字(元素句柄的 3/4 号字) + `state`(4)，push 走
`sub_1047BA50`（就是 `std::vector<Record>::push_back`，`add dword ptr [edi+4], 18h` 是 stride 证据）。

## 3. 三条通用门（我们只实现了其中一条）

每个入口在做任何事之前都过同一组门：

```
if (!m_enabled)                                  return;   // this+28
if (!DB_Element::isOK(m_view))                   return;   // this+8，当前视图
if (m_muted)                                     return;   // this+29，Mute()/UnMute()
if (!DB_Element::climb(elem, NOUN_XGEOM).isNull())return;   // ★ XGEOMETRY 子树整体排除
if (DB_DB::type(elem->getDB()) != 1)             return;   // 只处理 DESI 库
```

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

**版本稳定性（部分）**：在 E3D 2.10 的 `Core3D.dll` 里按字节 `68 0A 7A 65 05` 搜索，
命中 `0x102C260D`（`sub_102C25F0`）——`IsSignificant` 的字段 id 跨 2.10/3.1 未变。
2.10 那份 IDB 没有符号名，其余三个 id 的跨版本核对留作待办。

> 这三个字段就是我们缺的那份数据。`scripts/e3d/dump_core_primary_list.py` 现成的 frida 通道
> （`db_get_element_info(noun_hash, field_id)`）换个 `FIELD_ID` 就能把三张全 noun 表导出来，
> 与 `tests/fixtures/core-primary-list-e3d31.json` 同规格入库。

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
  `m_granularityMode ≠ 0` 时改判：**只有带负实体成员的 primitive 才上卷**，否则就画它自己。
  （`m_granularityMode` 的设置点未追，是 `PartialUpdateDesiMgr` 对象 +0x20 的一个 int。）

## 7. `Members` 的三个 `SearchMode`

`Members` `0x1047E240` 是一个显式栈的遍历：`DB_Element::members()` 取直接成员，逐个弹出，
对每个成员读同样的三个字段（`0x5657A0A` / `0xA103E`∨`0xBBD5ADC` / `0x92663`），然后按 mode 决定
**是否收集**与**是否继续下潜**：

| mode | 命名 thunk | 收集 | 下潜 |
|---|---|---|---|
| 0 | `SignificantMembers` `0x1047E9C0` | significant 成员 | **只穿过 significant 成员** |
| 1 | `PrimitiveMembers` `0x1047E7E0` | primitive 成员 | 所有成员，全子树 |
| 2 | `NegativeMembers` `0x1047E650` | primitive **且** negative 的成员 | 只穿过 primitive 成员 |

返回值是 bool：`out` 非空即真。`GranularityExpansion` 的 `m_granularityMode ≠ 0` 分支正是靠
mode 2 的返回值判断"这个 primitive 参与布尔运算吗"。

## 8. `AncestorDeletes` / `AbsentPrimitives`：两条我们没有的收尾

**`AncestorDeletes(e, state)` `0x1047C060`** —— 仅在 `state ∈ {3, 4}`（删除类）时动作。
沿 owner 链上行，对满足某个 noun 条件的祖先（反编译在这里丢了判据，`HIBYTE(v17)` 是残留）
检查队列里有没有 state==2 的同一元素，没有就 push `(ancestor, 2)`。
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

## 9. `IsPending`：祖先抢占去重

`IsPending(e, state)` `0x1047DDD0` 在 `ModelToUpdate` 和 `NewModelToUpdate` 里做前置拦截：

1. 若 `e` 不是 primitive，先把判据换成 `SignificantOwner(e)`；
2. 然后**沿 owner 链逐级上行**，每级在队列里找匹配条目：
   - `state == 0`：找 state 0 的条目；找不到再看 `IsDecendantPresent(e, 1)` 和 `IsPresent(e, 2)`；
   - `state == 1`：只找 state 1 的条目；
   - `state == 3`：`IsPresent(e, 3) || IsPresent(e, 4)`。
3. 命中即返回真 → 本次变化**整个丢弃**。

我们今天的去重是 `BTreeMap<(action, target_refno)>` ——**同一个 refno** 才合并。
祖先已经排了整根重生成、后代还各排一条，在我们这边是两条工作项。

## 10. `Update()`：三遍 + 一次收尾

```
pass 1: for rec in queue:
            if rec.state <= 1:
                if !EraseModel(rec) && !Exists(rec, idlist): 队列里就地删除该条
            elif rec.state in {3, 4}:
                EraseModel(rec)
pass 2: for rec in queue: if rec.state <= 1: DrawModel(rec)
pass 3: for rec in queue: if rec.state == 5: EraseModel(rec)
Finish()
```

pass 1 那句"擦不动、清单里也没有 → 把条目删掉"是**不画不在视图里的东西**：
只有本来就有模型、或者在当前 ID 清单里的元素才会走到 pass 2。

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
| primitive + 负实体 | mode 2 命中 → 上卷到 significant owner | 无负实体感知 | ❌ 缺 |
| 块内成员 | significant 后代逐个 `Erase`（state 4） | 无 | ❌ 缺（可能留孤儿行） |
| 缺失图元回收 | `AbsentPrimitives` → state 5 → `Erase` | 无 | ❌ 缺 |
| 祖先抢占去重 | `IsPending` 沿 owner 链查队列 | 仅 `(action, refno)` 精确去重 | ❌ 缺（多做，不是漏做） |
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

## 14. 明确没查的

- `m_granularityMode`（对象 +0x20）由谁设、默认值是什么——两条粒度分支哪条是生产常态，未定。
- `AncestorDeletes` 里筛选祖先的 noun 条件（反编译丢了判据，需要读原始指令流）。
- `IsPrimitive` 为什么是两个字段的或——两个位各自的字典名未知。
- E3D 2.10 只核对了 `IsSignificant` 一个 id；其余三个跨版本未核对。
- `PostSetRefListAttribute`（core.dll `0x5EAA7A3` 处的符号）维护的 back-ref 表结构——
  这是 ADR-003 那条线，本轮没碰。
