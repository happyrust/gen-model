# 2026-09-15 core.dll / Core3D 模型更新机制：哪些属性触发几何重算、哪些只动方位、哪些不影响模型 —— ida-bridge 取证 + 增量更新完善计划

- 问题（用户原话）：「结合 ida bridge 继续分析 core.dll 里面的模型更新的机制。分析出有哪些属性会导致模型增量更新的、
  哪些属性会导致方位变化的、哪些属性会不影响模型生成的。将这些机制分析整理清楚，并结合当前的实现分析下一步要完善增量更新的计划。」
- 手段：ida-bridge 活桥，`core.dll` = `idalib-33060`、`Core3D.dll` = `idalib-49136`（均为 `D:\ida_scratch\plant3\*.i64`，E3D 3.1）。
  本文新增的地址全部本轮实读；noun / 属性 / 字典字段常量用 `ENHAS2` 逆函数解回词（`531441 + Σ code(cᵢ)·27ⁱ`，A–Z → 1..26、空格 → 0）。
- 承接（不重复取证，只引用）：
  `docs/evidence/…/2026-09-06-ida-core3d-change-evalat-cascade.md`（`change/EVALAT` 规则表，A–T；本文改它两处，§7）、
  `docs/2026-09-04_model-cascade-update-inventory.md`（十一类联动 + GETWORK 过滤器）、
  `docs/specs/core3d-partial-update-conformance.md`（DRAFT 侧 `PartialUpdateDesiMgr`，R5 说「core 这一层没有 TransformOnly」——本文说清那句只对 DRAFT 层成立）、
  `上下文/会话-2026-08-31-core增量机制活桥取证与架构设计-800a23b3.md`（五层管线；它说 `changesBetweenSessions` 完全不看 CACHID——本文更正）、
  `teach/cases/case-02-transform-only-was-too-wide.md`（DCHC 四档的字典语义）。
- 我方对照的代码：`src/data_interface/model_impact.rs`（五张效果表 + DCHC 快照）、`src/data_interface/window_root_plan.rs`（direct 窗口规划器：元数据门 / 刚体前移候选）、
  `vendor/e3d-model/src/{element_diff,increment,ledger}.rs`（L2/L3/L4）、ADR-066（刚体前移）。
- ★0916 本份是 `gen-model-refactor` 工作树同名文件（提交 `898030df2`，S1–S8 已回填）取到 `gen-model-model-cache` 的副本，§0–§8 原样；
  **本轮新增 §9**（会话内交互路的 C++ 入口 `DESDRA_SCPlugs` 与 `descases/VDESPT` 的属性专项规则、QCHGLS 全部写入者、`UPGRPH` / `IDCHNG` 逐指令核、`PLCDEL`），
  §8 里被本轮结案的条目就地划掉。引用的 `docs/evidence/2026-09-06-ida-core3d-change-evalat-cascade.md` 与
  `docs/plans/2026-09-15-attribute-change-class-alignment-dev-plan.md` 仍在 `gen-model-refactor` 工作树（evidence 按政策不入库）。

---

## 0. 一句话结论

**core 对「一个属性变了要不要动图形、怎么动」的完整答案分三条通路，粒度从细到粗：**

1. **会话内交互路**（用户在 E3D 里改属性）：按属性字典的 **DCHC** 类别走 `change/EVALAT` 规则表进 QCHGLS，
   再由 3D 画列表的 **`change/UPGRPH`** 按类别执行——**类别 3（`POS`/`ORI`/`BFORI`）只改图形段矩阵（`css/STRTRA` / `css/SEGTRA`，不重建几何）**；
   **类别 4（315 个属性）重建**——顶层件整件 `add/REPELE`（删 + `ADDDES` 重生成），顶层件之下的**单个图元只重建它自己那一段**（`change/SADREP` → `css/SEGREP`）；
   类别 1/2（管件头 / 尾连通性，11 个属性）自身只重刷颜色，靠 `EVALAT` 转成类别 4 落到前一元件 / 自身 / 所属分支；
   **类别 0（3941 个属性）什么都不做**。★ 这是本轮最重要的新取证：**core 的 3D 侧确有「只动方位」的便宜路径**，
   核对表 R5「core 这一层没有 TransformOnly」只对 DRAFT 侧 `PartialUpdateDesiMgr` 成立。
2. **GETWORK / UNDO / REDO 路**（别人 SAVEWORK 了、我把变更收进来 —— 与 gen-model 的场景同构）：**元素级、不看任何属性**——
   `DB_RawChanges` 只走 `DB_IndexTableCompare`（记录被重写过就算），每个变更元素 `FNDTOP` 折到顶层可画件后以 **REDRAW=4** 进 `EVALAT`，
   顶层件整件 `REPELE` 重建。改个 `NAME` 也整件重画。唯一的属性门是 `hasElementChangedBetween` 对 `TUBI` 的四属性硬门（`POS`/`ORI`/`ITLE`/`SPRE`）。
3. **图形缓存路**（`DES_DrawListManager::cacheGetworkChanges(true)` 开着时）：修改类变更**只认 `CACHID` 这一个属性**
   （`hasTopLevelGraphicsChanged` 逐指令：`attributesChangedBetween` 结果里含 `ATT_CACHID` 才算变；`DES_DrawListCompare::attChange` 同款，
   而且它的 `includeFirst/After`、`reorder*` 回调全是空函数）——core 把「几何到底变没变」的判断整个交给了写 `CACHID` 的那一方。

对 gen-model 的意义：我们的场景是第 2 条路（跨会话文件差分），**core 在这条路上比我们粗**（整件重画、不分属性）；
我们做的元数据门与刚体前移是在第 1 条路的语义上做细分。所以**第 1 条路的 DCHC 类别 + `UPGRPH` 执行规则就是我们属性分类的权威 oracle**，
本文 §5 把三类属性按它列全，§6 逐条对我方现状，§7 给计划。**两个直接可落的差距**：
(a) 刚体前移（ADR-066，默认关）的排除规则比 core 的类别 3 例外少了三族（`UPGRPH` 的 23 个「几何由摆放位置派生」的 noun、`EVALAT` 规则 M 的 PANE/GENSEC/SCTN/BPANEL 上卷、
被属主消费的负体 / 开孔 / 型材附件），开门前必须补齐；
(b) 「不影响模型」我们只放行 5 个名字，core 会话内是 3941 个——差距可以用「生成读取集合」这第二票安全地缩小（e3d-model 只读 131 个运行库属性，其中 DCHC=0 的只有 6 个）。

---

## 1. 地址表（E3D 3.1，本轮实读）

### 1.1 core.dll（`idalib-33060`）

| 符号 | 地址 | 本轮读到的事实 |
|---|---|---|
| `DB_Attribute::dchc()` | `0x58cf550` | `this+8` 未加载则经 vtbl+20 懒加载，返回 `this+92`；**core.dll 内零调用方**（导出给外面用） |
| `ATAINT(attr, fieldHash, &out, &err)` | `0x58de220` | `DB_Attribute::findAttribute` + `DB_Attribute::getField(attr, fieldHash)`；属性不存在 → 错误 53 |
| `DB_Element::hasElementChangedBetween` | `0x593d6d0` | `NOUN_TUBI` 只问 `ATT_POS / ATT_ORI / ATT_ITLE / ATT_SPRE` 四个；其它 noun 走 `sub_5AAD8E0`（dab 记录级「任一属性变」）。2026-11-04 复核的调用方：`attributesChangedBetween`（`0x5928100`）、`rulesChangedBetween`、`DB_Compare::scanNew`、带元素名单的 `createListBetweenSessions`（`0x59833a0`）。`DB_Compare::checkEle` 不调用它 |
| `DB_RawChanges::createListBetweenSessions` | `0x59834b0` | `(DB*, 四个会话号)`。`switchToOldSession` → `DB_IndexTableCompare` → `iterate`（`0x5983c30`：按 `modified / inserted / 其余=deleted` 分三个 set）→ `getModifiedTypesAcrossSessions`。**只有索引级，没有任何属性判据**。`DB_DB::createGetworkEvents`（`0x58faef0`）调用的是这一重载 |
| `DB_RawChanges::createListBetweenSessions` | `0x59833a0` | `(dbnum, vector<DB_Element>, 四个会话号)`。对名单里每个元素调用 `hasElementChangedBetween`，为真才分创建 / 删除 / 修改。调用方是 `DBSaveWorkAllowed`、`DBFlushAllowed`、`PostSaveworkFailedAction`、`DB_DB::quitWork`，不是 GETWORK |
| `DB_DBPlugger::PostLocalChanges` | `0x591b5f0` | 调用方：`DB_DB::quitWork`（`0x590f5bc`）、`DB_MDB::quitWork`（`0x59f872f`）、`DB_Undo::nextMark / prevMark`（`0x59fb5a2` / `0x59fb7af`）、`sub_5A42C80`（`0x5a4329a`，同函数还调 `Pre/PostDBFileChanges`） |
| `ATT_CACHID` | `0x641d9a4` | core.dll 内唯一 xref 是初始化那一下写（`0x5667afa`，`sub_5667A70`）；**零次读** |
| `nounlib/LPRMTV` | `0x561ce28` | `ATNLOG(noun, 659518 = PRMF)`；不是 primitive 且 noun ≠ `WORL` 时，owner 是 `TMPL` 且 noun ∉ {`DDSE` `DPSE` `PORS` `LNKS`} 也算 primitive |
| `nounlib/LISTOP` / `LPOINT` | `0x561c93c` / `0x561cd78` | 顶层可画件 / 点类 noun 谓词（0906 已读） |
| dabacon 名表 `off_6003900` | — | `3 DBEFOR`、`37 DDEST`、`43 DFIND`、`44 db_move_down`、`46 db_find_reference`、`183 DRETUR` |

### 1.2 Core3D.dll（`idalib-49136`）

| 埋点名 / 符号 | 地址 | 角色 |
|---|---|---|
| `change/EVALAT` | `0x1022C679` | 联动规则表（0906 已读）。入口门 `UQGRAF() ‖ sub_1050DCA0()`；类别：`331445106` → 4、`73767168` → 1、其它 → `IDCHNG(attr)`；**类别 0 直接返回** |
| `change/IDCHNG` | `0x1022E302` | `ATAINT(attr, 596407 = "DCHC")` → 类别（读失败 → 0）；**再** `ATAINT(attr, 652066 = "PLCF")`，`== 1` 才置全局 `dword_10E98500 = -1`（INTUBE 旗标；0906 写成「类别 == 1 时置」，见 §7） |
| `change/EVALCD` / `change/EVALST` | `0x1022E020` / `0x1022E0A7` | 写 QCHGLS（`dword_10E98540`）：步长 3 扫，同 ref 取 **max** 类别，否则追加 `(ref₀, ref₁, 类别)` |
| `change/UPDATN` / `UPDATD` | `0x1022E3E7` / `0x1022E5AC` | 图形开着且不在 UCB → `DES_DrawListManager::updateGraphics()`（`0x1052D330`）→ `UpdateChangeList`（`0x1052B3C0`）→ 逐画列表 `change/UPGRPH` |
| **`change/UPGRPH`** | **`0x1022EAEE`** | ★ 本文主角：QCHGLS 每条 `(ref, 类别)` 的**执行**语义（§4） |
| `change/UPCACP` | `0x1022FCD0` | 全局 / ARF 列表的收尾（本轮未读体） |
| `add/ADDREM` → 回调 `add/REPELE` | `0x1021B2A7` → `0x1021BEBE` | **整件重建**：`REPELE` = `add/LADALL` 判在列表 → `build/TOPDEL`（`0x10250E74`，删顶层段）→ `add/ADDDES`（`0x1021D005`，重新走 ELMODL 生成）；`ADDREM` 还把 `unk_10E87E34` 队列里的附带 ref 一并 `REPELE` |
| `add/REPALL` | `0x1021BD40` | 全部重画（目录件 / 几何集变了） |
| `add/LADDED` | `0x1021DB75` | 「这个元素当前有图形（在画列表里）」 |
| **`change/TRASCN`** | **`0x1022E85F`** | 「transform scan」：`DDEST(-1)` + `DBEFOR` 后 `DFIND NEXT` 逐个走完元素之下的成员（`DRETUR` 回上层继续，直到错误 13），每个 `LISTOP` 为真的后代 → `css/STRTRA`（只改矩阵）；刷新观察者要求时才 `ADDREM` |
| **`css/STRTRA(ref)`** | **`0x10232512`** | 顶层段**只改矩阵**：`WSTRAM`（世界矩阵；`IFCOMP` 非 `PALJ` → `CLIMBA(POS)` 到属主；`IPFCOM` → 22-noun `CLIMBM`；`IHLCOM` → 单位阵 + `VUNIT`）→ `MAT16` → `GLXIST(ref)` 段存在才 `GLGOTO(ref)` + **`GLSMAT`**。调试串 `"Transform segment Refno $S "` |
| **`css/SEGTRA(ref)`** | **`0x1023378B`** | 图元段**只改矩阵**：`INCOMP ‖ PIPCA` → `desdblib/LVTRAM(负)`，`IPFCOM` → `desdblib/GPFSTR`，其它 → `LVTRAM` → `MAT16` → `GLGOTO` + `GLSMAT` |
| `change/SADREP(top, flag, &neg, &out, ref)` | `0x1022FB1A` | `flag` ? `change/SOPNDM` : `css/SEGOPN(top)` 打开顶层段，再 **`css/SEGREP(ref)`**（`0x102334D0`）：**只重建 `ref` 这一个图元的段** |
| `css/SEGOPN` | `0x102331C7` | 段是否存在 / 打开 |
| `change/COLRES` / `aids/RMKELE` | `0x1023106B` / `0x101DA626` | 颜色 / 辅助图元重置（不动几何） |
| `desdblib/FNDTOP` / `LVTRAM` / `GPFSTR` | `0x10380E38` / `0x10381058` / `0x10381990` | 上卷到顶层件 / 图元相对顶层的局部矩阵 / 型材附件矩阵 |
| `sub_1050D2E0(mode, ref)` → `DES_DrawListRefresh::beforeChanged` | `0x1050D2E0` → `0x1052E0B0` | 问注册的 `DES_DrawListRefreshObserver` 们「这次变化要不要整件重加」；**无观察者时恒假**；`promoteARFCache_` 为真时不问 |
| GETWORK 路 `DDES_GraphicsMngr::PostLocalChanges` | `sub_1052E9F0`（vtable `0x10b30454`） | 0904 §2.1；`op` 4/5/6 = postGetWork / postUndo / preRedo → `sub_1052F2E0` 过滤 → `FNDTOP` → `EVALAT(top, REDRAW)` |
| 缓存路 `DES_DrawListManager::PostLocalChanges` | `0x1052AEA0`（vtable `0x10b30158`） | `op == 4 && this+9` 才干活；`this+9` 由 `cacheGetworkChanges(true)`（`0x1052B690`，同时 `DB_DBPlugger::addPlug`）置位。修改集逐个 `hasTopLevelGraphicsChanged` |
| `DES_DrawListManager::hasTopLevelGraphicsChanged` | `0x1052C850` | **逐指令**：`previousSession` 成功 → `attributesChangedBetween(extract(prev), prev, extract(cur), cur, &vec, false)` → 遍历 `vec`，`== ATT_CACHID` 才 `bl = 1`；否则 0 |
| `DES_DrawListManager::changesBetweenSessions` | `0x1052B730` | 对每个 `SITE`/`*WLD` 建 `DES_DrawListCompare`（ctor `0x1055FE80`）→ `DB_Compare::scan` → 三个 set 分别 `processDeletes / processCreates / processModifications`（各自 `findTopLevelElement`） |
| `DES_DrawListCompare` vftable | `0x10b34458` | `scan` / `checkElement`(空) / `deleteAll` ✔ / `createAfter` ✔ / `createFirst` ✔ / `createNull`(空) / `reorderAfter`(空) / `reorderFirst`(空) / **`includeAfter`(空) / `includeFirst`(空)** / **`attChange`：`findAttribute(attr) == ATT_CACHID` 才收** / `ruleChange`(空) / `insert|removeFromSecondaryList`(空) |

---

## 2. 三条通路总图

```text
                     ┌── 会话内交互（DCHC 驱动，最细）────────────────────────────────────────────────┐
 descases/VDESPT 属性 put ─┐                                                                              │
 descases/VDESDL 删除     ─┤  change/EVALAT(el, noun, attr)                    change/UPDATN            │
 descases/SCINC2 include  ─┼─▶  类别 = REDRAW→4 | INTUBE→1 | IDCHNG(attr)=字典 DCHC  ─▶ EVALCD/EVALST ─▶ QCHGLS ─▶ updateGraphics ─▶ UPGRPH(ref, 类别)
 change/EVALNW  新建      ─┘    类别 0 → 直接返回（什么都不做）                    (同 ref 取 max)              │      │
                                规则 A–T：上卷 / owner / 成员 / 结构连接 / 引用表……                                  │      ├ 类别 3 → STRTRA / SEGTRA（只改段矩阵）
                                                                                                                     │      ├ 类别 4 → REPELE 整件 / SEGREP 单图元 / REPALL 全部
                     ┌── GETWORK / UNDO / REDO（元素级，最粗）───────────────────┐                                    │      └ 类别 1/2 → 只 COLRES（几何靠 EVALAT 转成的类别 4 条目）
 DB_RawChanges(DB_IndexTableCompare: 记录被重写=修改) ─▶ DDES_GraphicsMngr::PostLocalChanges(op 4/5/6)
        ─▶ sub_1052F2E0（gb 门、WORL/MNUM）─▶ FNDTOP(el) ─▶ EVALAT(top, noun, REDRAW=331445106) ─▶ 同上，类别恒 4 ─▶ UPGRPH: top==ref → ADDREM/REPELE 整件重建

                     ┌── 图形缓存路（cacheGetworkChanges 开着时才挂上）──────────┐
 DES_DrawListManager::PostLocalChanges(op 4) ─▶ 修改集逐个 hasTopLevelGraphicsChanged(el) = 「CACHID 变了」 ─▶ processModifications
 DES_DrawListManager::changesBetweenSessions ─▶ DES_DrawListCompare（attChange 只收 CACHID；include/reorder 全空）
```

三条路的「属性判别力」：交互路 = 字典 DCHC 逐属性；GETWORK 路 = 无（TUBI 四属性门除外）；缓存路 = 只看 `CACHID`。

★0916 交互路左端补齐（§9.1）：`descases/VDESPT` 不是命令层，而是 **`DB_Element::putAtt` → `internalPutAtt`（同值早退）→ 写 →
`DB_Element::postSetAttribute` → `DB_ElementChangesPlugger::PostSetAttribute`（vtbl+52）→ 订阅者 `DESDRA_SCPlugs::PostSetAttribute`（非 PADD 库）→ `VDESPT`**。
所以 PML / .NET / 表单 / 规则引擎——任何走 `putAtt` 的属性写都进这条路；`VDESPT` 自己先查一次 DCHC ≠ 0 再进 `EVALAT`，另带一条 `PLOO.HEIG → owner` 的上卷（规则 U）与几笔属性写回。
QCHGLS 除 `EVALAT` 外还有 8 个写入者，全是结构变化或命令处理器，**属性触发的只有 `VDESPT → EVALAT` 一条**（§9.3）。

---

## 3. 属性 → 变化类别：字典 DCHC 与 IDCHNG 的两个字段

### 3.1 字典四档（`src/data_interface/dchc_change_classes.json`，4270 条；运行库 1878-noun 表里出现的属性名 4478 个）

| DCHC | 字典条数 | 运行库出现 | 成员 | 语义（core 交互路的执行，见 §4） |
|---:|---:|---:|---|---|
| **0** | **3941** | 3657 | `NAME` `DESC` `PURP` `FUNC` `LCHKDA` `NGMR` `PKEY` `STYP` `GTYP` `HREF` `TREF` … | **无设计变化**：`EVALAT` 直接返回，图形一动不动 |
| 1 | 7 | 7 | `HBOR` `HDIR` `HPOS` `HSTU` `HZAXI` `LHEA` `TZAXI` | 元件**头端**连通性：规则 H → **前一个元件**记类别 4（它的隐含管段变了）；自身只 `COLRES` |
| 2 | 4 | 4 | `LTAI` `TBOR` `TDIR` `TPOS` | 元件**尾端**连通性：规则 I → 自身记类别 4 |
| **3** | **3** | 3 | **`POS` `ORI` `BFORI`** | **刚体位姿**：`UPGRPH` 只改段矩阵（例外见 §4.1） |
| **4** | **315** | 293 | `DIAM` `HEIG` `XLEN` `SPRE` `CATR` `CREF` `JOIS` `LEVE` `TVIS` `POSS` `POSE` `CPOS` `YDIR` `ZDIR` `PARA` … | **几何 / 表示变了**：重建（整件 / 单图元 / 全部，按 §4.2） |
| （无） | — | 514 | UDA、伪属性（`CLIST` `ELCXRF` `CACHID`……）、字典没收的名字 | `IDCHNG` 的 `ATAINT` 读失败 → 类别 0 → 不动 |

两个强制码不查字典：`REDRAW`（`331445106`）→ 4、`INTUBE`（`73767168`）→ 1（ADR-002 已记）。
运行库 4478 个属性名按 DCHC 分布：`0: 3657 / 无: 514 / 4: 293 / 3: 3 / 1: 7 / 2: 4`。

### 3.2 `IDCHNG` 读的是**两个**字典字段（本轮改正 0906 的一句）

```
IDCHNG(attr):
    (cls, err) = ATAINT(attr, 596407 /* "DCHC" */)      ← 0x10B23D70
    if err: return 0
    (plcf, _) = ATAINT(attr, 652066 /* "PLCF" */)      ← 0x10B23D74
    dword_10E98500 = (plcf == 1) ? -1 : 0               ← INTUBE 旗标（UPDATD 收尾用它调 PLCDEL）
    return cls
```

0906 文档写「`==1`（INTUBE）时置全局」——旗标实际来自**另一个字段 `PLCF`**，与 DCHC 类别 1 不是同一件事。~~`PLCF` 我们没有导出（ADR-008 导出器只提炼了 DCHC），§7 P0 补。~~
★0915 S1 已导出：`src/data_interface/plcf_flags.json`（`gen_dchc_fixture.py` 与 DCHC 同批提炼），字典 4270 条里 **58 个 `PLCF == 1`**——
位姿三个 `POS ORI BFORI`（DCHC 3）、54 个 DCHC = 4（`SPRE HEIG PARA DESP CREF JOIS JOIE CFRA PFRE CUTB CUTP POSS POSE NPOS YDIR ZDIR PX PY PZ` 与楼梯 / 栏杆 / 桥架尺寸 …）、
以及一个 DCHC = 0 的 `CLGREF`；DCHC 1 / 2 的头尾连通属性**一个都不置**（它们走 `INTUBE` 强制码那条路）。`model_impact::raw_plcf(name)` 三态，只作报表。

### 3.3 类别在 `EVALAT` 里的去向（只列与类别有关的分支；全表见 0906 §2.2）

| 类别 | `EVALAT` 里的处置 | 落到 QCHGLS 的条目 |
|---|---|---|
| 0 | 入口 `if (v16)` 不成立，**什么都不做** | 无 |
| 1 | 规则 H：`CRETUR` + `NATTA(PREV)` → 前一元件 | `(prev, 4)`；自身按默认 / 规则 L 记 `(self, 1)` |
| 2 | 规则 I | `(self, 4)` |
| 3 / 4 | 规则 R（`PLOPEN` `DPCA` `DPCY` `DPSP` `DPSE` → `FNDTOP`）、规则 S（`LPOINT` 点类 → `FNDEQP` 所属几何件） | `(top 或 几何件, 4)` |
| 任意 ≠ 0 | 规则 J/P（`HOLC` 孔洞 / `PIPC` / `HNGC` 元件、`PLTGRD` `INTFRM` → **owner**）、规则 M（自身或祖先 ∈ {`PANE` `GENSEC` `SCTN` `BPANEL`} → 那个结构件）、规则 N/O/Q/T、规则 L（`LISTOP` → 自身，**类别继承**） | `(owner / 结构件 / top, 4)`、`(self, 继承类别)` |

所以一个 **BOX 在 EQUI 下改 POS**：EQUI 不是 BOX 的 PANE/GENSEC 类祖先、BOX 不是元件也不是点 → 默认目标 = 自身、类别继承 → `(BOX, 3)`。
一个 **管件（IPCOMP）改 POS**：规则 P → `(BRAN, 4)`，外加 `(管件, 3)`。一个 **PAVE 顶点改 POS**：规则 S → `(PANE, 4)`。
一个 **ZONE 改 POS**：没有任何规则命中、`LISTOP(ZONE)` 假 → `(ZONE, 3)`。

---

## 4. ★ `change/UPGRPH`：类别在 3D 画列表上的执行语义

对 QCHGLS 每条 `(ref, cls)`（伪码 + 常量解码；调用序与常量已核，分支结构未逐指令核，§8）：

```text
DGOTO(ref); noun = TYPE
top   = FNDTOP(ref, flag = (noun == TUBI))          # TUBI 上卷跨过 BRAN 那一级（0831 已记）
（与上一条同 top 且 top ≠ ref 且 top 非空 → 跳过：同一顶层件只处理一次）
topNoun = TYPE(top)（top 走不到 → NULL）
added = add/LADDED(top)                              # 顶层件现在有图形吗
if cls > 0 and beforeChanged(mode 1, top) and added: ADDREM(top, REPELE)   # 刷新观察者要求才整件重加（默认无观察者）

if cls == 3:                                                          # ★ 位姿
    if noun ∈ P23（下表）:            if added: ADDREM(top, REPELE)；TRASCN 同款走子树 RMKELE   → 整件重建
    elif top == ref:                  TRASCN(); if added: SEGOPN(-ref) ? STRTRA(-ref) : STRTRA(ref)   → 只改矩阵（先给子树里的 TOPF 后代改矩阵）
    elif LPRMTV(noun):                if added: SEGTRA(ref)                                          → 只改这个图元的段矩阵
                                        if IPCOMP ‖ IHCOMP: prev = NATTA(PREV).REF；SADREP(top, -prev)；SADREP(top, -ref)  → 前后两段隐含管段重建
    else:                             TRASCN()                                                        → 容器（ZONE/STRU/…）：子树里每个 TOPF 后代 STRTRA
elif cls == 4:                                                        # ★ 几何
    if ICCOMP ‖ IGMCOM ‖ INGCOM ‖ IG2COM ‖ IPLCOM ‖ IPTCOM ‖ noun ∈ {SPCO, SVER, DATA}:  REPALL()       → 目录件 / 几何集变了：全部重画
    elif top == ref ‖ topNoun == TMPL: if added: ADDREM(top, REPELE)；TRASCN()；SEGOPN/STRTRA        → 顶层件整件重建
    elif LPRMTV(noun):                if added: IPCOMP ‖ IHCOMP ? SADREP(top,-prev)+SADREP(top,ref)+SADREP(top,-ref) : SADREP(top, ref)   → 只重建这个图元（和它两侧管段）的段
    else:                             TRASCN()
else (cls 1 / 2):                     （不动几何）
收尾（各分支）：COLRES(ref)；if added: RMKELE(top) 并沿成员两层 RMKELE   → 颜色 / 辅助图元
```

**P23 —— 类别 3 也整件重建的 23 个 noun**（`0x1022EAEE` 内联常量，逆 hash）：
`DRAW` `CLNTIL` `STRFLT` `INFITT` `PLTFRM` `ELFITT` `STRLNG` `HVACFI` `RAIL` `DOOR` `WINDOW` `FPFITT` `LINDIM` `FLRCOV` `MLABEL` `FIXTUR`
`HRPOST` `HRTERM` `HRGATE` `HRPANE` `KICKPL` `CTSUPP` `FURNIT`。
共同点：**几何由摆放位置派生**——门窗在墙上开孔、吊顶件按网格摆放、栏杆立柱 / 端头 / 门 / 踢脚板随相邻件、桥架支架随托盘、标注随被标对象、楼梯梯段 / 斜梁。

### 4.1 类别 3 = 只改矩阵，四条例外

| 元素形状 | core 的动作 | 我方对应（§6） |
|---|---|---|
| 顶层件自己（`top == ref`，如 EQUI / PANE / SCTN 挪位） | `TRASCN`（子树里嵌套的 TOPF 各 `STRTRA`）+ 自身 `STRTRA`：**全部只改段矩阵** | 刚体前移 `transform` 桶 ✅ |
| 容器（ZONE / SITE / STRU / FRMW …） | `TRASCN`：**子树里每个 TOPF 后代只改段矩阵**——core 自己的「刚体前移」 | 刚体前移「锚点在根之上」✅ |
| 顶层件之下的普通图元（BOX 在 EQUI 下） | `SEGTRA(ref)`：只改**这个图元**的段矩阵 | 刚体前移对记录逐条左乘 Δ ✅（比 core 还细：按记录） |
| 例外 ①：管件 / 支吊架元件（`IPCOMP` / `IHCOMP`） | `SEGTRA` 自身 + `SADREP` 前后两段隐含管段 **重建**；且 `EVALAT` 规则 P 另记 `(BRAN, 4)` → 分支整件重建 | 路由容器排除 → 整根 `regen` ✅（更粗） |
| 例外 ②：P23 那 23 个 noun | `ADDREM/REPELE` 整件重建 | ❌ **无对应排除**（§6.2 G1） |
| 例外 ③：点类（规则 S）、`PLOPEN`/`DP*`（规则 R）、被 PANE/GENSEC/SCTN/BPANEL 消费的成员（规则 M）、孔洞 / 元件（规则 J/P） | `EVALAT` 已转成类别 4 落到**所属几何件 / owner / 结构件** → 重建 | 点 / 环容器排除 ✅；**规则 M / J 那两族 ❌**（§6.2 G2） |
| 例外 ④：刷新观察者（`DES_DrawListRefresh` 回调）要求整件重加 | `ADDREM/REPELE` | 应用层钩子，无对应物（⚪） |

### 4.2 类别 4 的三种粒度

| 变更元素 | core 的动作 | 粒度 |
|---|---|---|
| 顶层件自己，或 owner 是 `TMPL` | `REPELE`：`TOPDEL` 删段 + `ADDDES` 重新 ELMODL | **整件** |
| 顶层件之下的图元（`LPRMTV`） | `SADREP(top, ref)` → `SEGREP`：只重建这一个图元的段（管件另带两侧管段） | **单图元** |
| 目录件 / 几何集（`ICCOMP` `IGMCOM` `INGCOM` `IG2COM` `IPLCOM` `IPTCOM`、`SPCO` `SVER` `DATA`） | `REPALL` | **全部** |
| 其它（容器上的类别 4 属性，如 ZONE 的 `LEVE`） | `TRASCN` → 子树 TOPF 只改矩阵 | 不重建几何 |

### 4.3 GETWORK 路：任意记录重写 → 顶层件整件重建

GETWORK 走 `DB_DB::createGetworkEvents`（`0x58faef0`），它调用的是 `createListBetweenSessions` 的 `(DB*, 四个会话号)` 重载（`0x59834b0`）。这一重载只有 `DB_IndexTableCompare`：记录在会话区间里被重写就是「修改」，
没有 `checkEle`、没有 `hasElementChangedBetween`；`sub_1052F2E0` 过 gb 门后 `FNDTOP` 折到顶层件，`EVALAT(top, REDRAW)` 类别恒 4，
`UPGRPH` 走 `top == ref` 分支 → `REPELE` 整件重建。**改 `NAME`、改 `DESC`、甚至内容一字未变只是被重写，顶层件都整件重画。**

`hasElementChangedBetween`（TUBI 四属性）不在这个重载里。它的调用方是 `attributesChangedBetween`（`DB_DB::elementsChangedBetween` 用它）、`rulesChangedBetween`、`DB_Compare::scanNew`，以及带元素名单的另一重载 `0x59833a0`。`0x59833a0` 的调用方是 `DBSaveWorkAllowed`、`DBFlushAllowed`、`PostSaveworkFailedAction`、`DB_DB::quitWork`，不是 GETWORK。`DB_Compare::checkEle` 本身不调用 `hasElementChangedBetween`。

### 4.4 缓存路：`CACHID` 是 core 自己的「几何版本戳」，但写它的人不在 core.dll / Core3D 里

- `hasTopLevelGraphicsChanged`：`attributesChangedBetween` 的结果里含 `ATT_CACHID` → 真，否则假（逐指令，`0x1052c917`–`0x1052c92d`）。
- `DES_DrawListCompare`：`createFirst / createAfter / deleteAll` 收元素，`attChange` **只收 `CACHID`**，`includeFirst / includeAfter / reorder* / ruleChange / *SecondaryList` **全是空函数**——
  改挂、换序在这条路上都不算图形变化，除非 `CACHID` 也变了。
- 运行库 1878-noun 表里 **204 个 noun 带 `CACHID`（`WORD` 型）**；真库见过它被写（db8000 24→25 `EQUI 24384/24778` `CACHID` "" → "1"，`e3d_collector.rs` 记着）。
  core.dll 只在初始化写 `ATT_CACHID` 指针、零次读；Core3D 两处读（上面两个）。**写值的一方在别的 DLL（Design 应用侧），本机没有它的 IDB**（§8）。
- 对我们的意义：它是 core 把「几何变没变」的判断**外包**给写方的证据——与我们「用凭证 / manifest 记几何版本」是同一个想法；
  但它只在缓存模式下被消费，不能当成语料里可靠的「几何变了」信号（§7 P4 建议先普查它的一致性再定用途）。
- ★0916 S8 普查结论（`window_root_plan::tests::probe_cachid_consistency`，453 个 DESI 库 / 2961 窗 / 36162 个真变了的 Modified 候选，
  证据 `docs/evidence/2026-09-16-cachid-consistency-corpus.md`，按政策不入库）：
  (1) **值是计数不是旗标**：13724 次变化全是 `+1`（`0→1` 5241、`n→n+1` 8483，没有别的形状）；它是 `WORD` 型、无反哈希文本，原始字就是计数，
  上面那句 `"" → "1"` 其实是 `0 → 1`。
  (2) **写方按应用分**：带 `CACHID` 的 26627 个候选里只有 13753 个非 0；写过的只有 `GENSEC HANG PANE BRAN SUBE EQUI SCTN HANDRA TRNCUR STRFLT
  GENPRI PNOD SUBS FLOOR TMPL STRU TRNPRI` 十七个 noun，船体（`STWALL GWALL HRSOF CWALL WALL CFLOOR CSTIFF HPLATE HSTIFF`）、`CABLE`、栏杆件
  （`HRPOST HRTERM HBRCKT`）、`CWBRAN`、`VOLM` 上一次都没写过——写它的是管道 / 设备 / 结构（型钢 / 板 / 支吊架）应用，船体与电缆应用不写。
  (3) **语义是「顶层件的图形缓存版本」，不是「本元素存储几何变了」**：13724 次跳动里只有 369（2.7%）伴随本元素 DCHC≠0 / 结构变化，621（4.5%）
  伴随只 DCHC=0 / 字典外 / 成员表的变化（成员增删 132/132 跳动——子树图形变记在属主身上），**12734（92.8%）除它之外一个字没变**，其中 12476 连
  子树里都没有任何同窗变化：形状是整库扫写（aps7343 6→7 全部 SCTN / PNOD / GENSEC `0→1`；GENSEC 2794 个 `0→1` 再 2794 个 `1→2`；HANG 1266 × 3 轮），
  是应用侧重建缓存的会话，不是几何编辑（从形状推的，见 §8）。
  (4) **单向 oracle 成立、反向不成立**：在有值的元素上「有 DCHC≠0 变 ⇒ `CACHID` 变」369/373 = **98.9%**（4 个反例全是 BRAN 头尾类别 1/2 属性）；
  没写过的元素上 213 个 DCHC≠0 变化（类别 4 173、1/2 37、3 3）停在 0，是覆盖问题不是纪律问题。反向「`CACHID` 变 ⇒ 几何变了」只有 2.7%，不能当信号。
  (5) **对我方三档的副作用**：字典没收它，混进差分就既不放行也不刚——有值元素上 88 次含 `POS`/`ORI` 的位姿变化**全部**同时跳动（`is_placement_only`
  因此为假，其中 84 次若忽略它本可 transform）；只 DCHC=0 的 83 次跳动（`LCHKDA` 81/83、`HREF` 4/5、`TREF` 1/1、`NAME` 2/25）里 67 次从 skip 推成
  regen；12734 个只 `CACHID` 的候选今天全部上卷重算。用途照 §7 P4：**不接线**；两处待拍板见开发计划 §8 Q5 / Q6。

---

## 5. 三张答案表

### 5.1 只导致方位变化（transform-only）

| 属性 | DCHC | core 交互路 | 条件 / 例外 |
|---|---:|---|---|
| **`POS`** | 3 | `STRTRA` / `SEGTRA` / `TRASCN` 只改段矩阵 | 元素 **不是**：P23 noun、管件 / 支吊架元件（`PIPC`/`HNGC` 位：分支整件重建）、点类（`POPF`：所属几何件重建）、孔洞（`HOLC`：owner 重建）、`PLOPEN`/`DP*`（顶层件重建）、被 PANE/GENSEC/SCTN/BPANEL 消费的成员（结构件重建）、`FPFITT` 等 owner 是 `CLNTIL` 的（owner 重建）、`SUBCOM`（祖父重建）、`FIXING`（顶层 FIXING 重建） |
| **`ORI`** | 3 | 同上 | 同上 |
| `BFORI` | 3 | 同上 | 运行库只有 1 个 noun 带它；我方未收（落 `Unknown` → 重算，方向安全） |

**没有第四个。** `POSS`/`POSE`/`CPOS`/`NPOS`/`POSL`/`YDIR`/`ZDIR` 都是类别 4（案例 02 的收窄与字典一致，本轮从 `UPGRPH` 侧再证一次：类别 4 走 `REPELE`/`SEGREP` 重建）。
在 GETWORK 路上**这三个也整件重建**——core 只在会话内才把位姿当便宜路。

### 5.2 导致几何重算（regen）

1. **DCHC = 4 的 315 个属性**（运行库出现 293 个）。我方五张表命中 102 个，其余 **213 个落 `Unknown`**（方向正确：`Unknown` 也重算；但报表上都显示成「未知属性」，§7 P0 建议整表导入。
   ★0915 S1 已导入：第六张表 `dictionary_change_class_attr_names.json`，213 + 下条的 4 个 = 217 条；其中 18 个 schema `ELEMENT` 引用登记 `DependencyCascade`、其余 `DirectGeometry`，判决不变）。
   运行库出现而我方没登记的 191 个里，成簇的是：结构连接 / 型材（`CFRA` `PFRE` `JOIS` `JOIE` `JLNS` `JLNE` `CUTB` `CUTP` `CTYS` `CTYE` …）、可见性 / 表示（`LEVE` 已有；`TVIS` `BVIS` `LVIS` `ORIF` `DISP`）、
   楼梯 / 栏杆 / 平台族（`HEADH` `RAILHE` `TREADD` `RISE` `OGOING` `POSTSP` …）、桥架 / 网格（`TRAYW` `TRAYH` `GRD*` `CTTYP`）、电缆（`STAREF` `ENDREF` `CBSTA`）、板 / 墙（`WTHICK` `WJUSL` `PDSHAP`）。
2. **DCHC = 1 / 2 的 11 个属性**：core 转成类别 4 落到前一元件 / 自身 / 所属分支（规则 H / I / P）。我方：`HBOR HDIR HPOS HSTU TBOR TDIR TPOS` 在 `DependencyCascade` → 重算 ✅；`HZAXI LHEA LTAI TZAXI` 落 `Unknown` → 重算 ✅。
3. **结构变化**：新建（`EVALNW` → REDRAW）、删除 / 改挂（`VDESDL` / `SCINC2`：先 INTUBE 再 REDRAW，删 / 改挂两端都算）、成员换序（`DB_Compare` 的 `reorder*`；GETWORK 路上记录被重写即算）。
4. **GETWORK 路上的一切记录重写**（含 DCHC=0）——core 在跨会话路上不分属性。

### 5.3 不影响模型生成（no-op）—— 分三层说

| 层 | 口径 | 数量 |
|---|---|---|
| core 会话内 | **DCHC = 0 的属性**：`EVALAT` 直接返回 | 字典 3941（运行库出现 3657）+ 字典没收的 514 个名字（UDA / 伪属性 / 长名） |
| core GETWORK | **没有**：任何记录重写都整件重画（TUBI 四属性门除外） | 0 |
| **我方（direct 路今天）** | 效果表 `DataOnly` **且** 字典 DCHC=0：`NAME` `DESC` `PURP` `FUNC` `LCHKDA`（加四条结构条件：没换类型 / 属主 / 成员 / 不 `opaque`）——`model_skip_metadata_only_changes` 默认开 | **5** |

中间那 3936 个 DCHC=0 属性，我方今天**全部整根重算**。但「core 不重画」≠「不是我们的几何输入」——
**e3d-model 读的 131 个运行库属性里有 6 个 DCHC=0**：`DIR`（`catalogue_point.rs`）、`FILL`（`section.rs`）、`GTYP`（`route.rs`）、`PKEY`（`section.rs`）、`UKEY`（`udet.rs`）、`VXREF`（`polyhedron.rs`）；
`model_impact.rs` 的表里另有 13 + 6 个 DCHC=0 却判为影响模型的名字（`FSPREF SCREF PSPE DKEY DDPR GTYP HREF TREF STYP CURD JFRE PKEY PTYP` / `SIZE MAXA NGMR ORRF PTOF VXREF`）。
这类属性在 E3D 里由专用编辑工具改、工具自己发 REDRAW，所以字典给 0；我们从文件差分看不见那次 REDRAW，**必须照旧重算**。
⇒ 「不影响模型」在我方的正确判据是 **DCHC = 0 ∧ 不在我们的生成读取集合里**（§7 P2），而不是 DCHC = 0 本身。

### 5.4 我方效果表 vs 字典（本轮对账，`D:\ida_scratch\plant3\_dchc_vs_tables.py` 一次性探针）

| 表 | 条数 | DCHC 分布 | 字典里没有的 |
|---|---:|---|---|
| `DATA_ONLY` | 6 | 0 ×5 | `CACHID`（伪 / 项目属性） |
| `STRUCTURAL` | 6 | 4 ×1（`LEVE`） | `OWNER CHILDREN NOUN TYPE LEVEL`（元素元数据，不是字典属性） |
| `TRANSFORM_ONLY` | 2 | **3 ×2** | — |
| `DEPENDENCY_CASCADE` | 59 | 0 ×13 · 1 ×4 · 2 ×3 · 4 ×26 | 13（`SPREF PSPREF GMREF DDANGLE DDHEIGHT DDRADIUS IPARAM CONN ADIR RDIR LDIR DETR PVER`） |
| `DIRECT_GEOMETRY` | 123 | 0 ×6 · 4 ×75 | 42（`ANGF ABOR LBOR SBOR IRAD ORAD CRAD THIC WIDE DEPT …`，多为字典长名的缩写 / 旧名） |

`TRANSFORM_ONLY` 与字典类别 3 逐位一致（`exemption_tables_match_the_dictionary_change_class` 钉着）；`DATA_ONLY` 可比的 5 条全是 0。两张「少算」表没有一条与 core 反向。

---

## 6. 与当前实现对照

### 6.1 现状：三档在 direct 路上的落点

| 档 | 判据 | 执行物 | 开关 / 默认 |
|---|---|---|---|
| Skip | `attribute_is_metadata_only`（效果表 DataOnly ∧ DCHC=0）× 全部变更属性 + 4 条结构条件（`window_root_plan::change_is_metadata_only`） | e3d-model `plan_update_with` 记 `no_model`，不上卷 | `model_skip_metadata_only_changes` **默认开** |
| Transform | `ElementDiff::is_placement_only`（只有 `POS`/`ORI` 变）→ `rigid_anchor`：`Δ = W_T·W_S⁻¹`；排除：候选是桥架件、祖先含 `CWBRAN` / 点环容器（gb==1）/ 路由容器；ADR-066 D1 另五条（未安定 / 退化 / 级联扩展进来 / 子树有非锚点候选 → 全部 `regen` 吞掉） | 投影记录左乘 Δ，根级 CAS 发布（`rigid_motion.rs`） | `model_transform_cheap_path` **默认关**（对拍门未过） |
| Regen | 其余一切（含 `Unknown`、UDA、`opaque`、结构变化） | 根级整根重算（ADR-056 D3） | — |

### 6.2 差距表（core 交互路 = oracle）

| # | core 的规则 | 我方现状 | 判 | 去向 |
|---|---|---|---|---|
| G1 | 类别 3 对 **P23** 23 个 noun 整件重建（几何由摆放派生） | 刚体前移无此排除。我们已生成的族里命中：栏杆 `HRPOST HRTERM HRGATE HRPANE KICKPL RAIL`（`handrail.rs`）、楼梯 `STRLNG STRFLT`、门窗 `DOOR WINDOW`、`CTSUPP`、`PLTFRM`（其余多为 gb==3 覆盖度问题，今天不生成）。★0915 S2 已补：e3d-model `category::PLACEMENT_DERIVED_NOUNS` + `window_root_plan::anchor_noun_exclusion` | ~~❌ 开门前必补~~ ✅ S2 | P1-a |
| G2 | 规则 M（自身 / 祖先 ∈ {PANE GENSEC SCTN BPANEL} → 结构件重建）、规则 J/P（`HOLC` 孔洞 / 元件 → owner 重建）、规则 R（`PLOPEN` `DP*` → 顶层件） | 刚体前移只排除点环容器与路由容器。一个 `PLOPEN` / 负体 `NBOX` / `FIXING` 挪位：它们是被属主**消费**的几何，锚点自己没有独立记录，根却被判进 `transform` → **属主网格上的孔停在旧位置**。★0915 S3 已补：`anchor_lineage_exclusion`——负体 / `ProfileData` / 几何集按自身 noun；非单元在 `consumes_members` 属主下；规则 M 结构件作严格祖先（FIXING 是单元，靠这一条） | ~~❌ 正确性缺口~~ ✅ S3 | P1-b |
| G3 | 类别 3 对管件：`SEGTRA` 自身 + 两侧管段 `SEGREP`；规则 P 另整根 | 路由容器排除 → 整根 `regen` | ✅ 更粗不漏 | — |
| G4 | 类别 3 对点类（规则 S）→ 所属几何件重建 | 点环容器排除 → `regen` | ✅ | — |
| G5 | 容器位姿 → `TRASCN` 子树 TOPF 只改矩阵 | 刚体前移「锚点在根之上」 | ✅ 同构（我们按记录、core 按段） | 开门 |
| G6 | 类别 0 → 不动（3941 个） | ~~只放行 5 个名字~~ ★0915 S6：放行「DCHC=0 ∧ 生成器不读」= 3941 − 6（`DIR FILL GTYP PKEY UKEY VXREF` 由读取集合挡住）；预览同口径 | ✅ 两票；多算只剩那六个（我们真读） | 已落地（S6） |
| G7 | 类别 4 图元级 `SEGREP`（只重建那一个图元的段） | 根级整根重算 | ◐ 多算（ADR-056 D3 的既定取舍；`execute_plan` 单元级只作规格） | P3（先量） |
| G8 | 类别 1/2 → 前一元件 / 自身 / 分支 | 分支整根重算覆盖 | ✅ | — |
| G9 | `REPALL`：目录件 / 几何集变了全部重画 | CATA → DESI 走依赖闭包 / 引用表反查（0904 #4；`2026-09-15-cata-dependency-closure-cache-dev-plan.md`） | ✅ 比 core 细 | 已有计划 |
| G10 | 213 个 DCHC=4 属性我方未登记 | 落 `Unknown` → 重算 | ◐ 行为对、报表噪音 | P0 |
| G11 | `IDCHNG` 的 `PLCF` 字段（INTUBE 旗标） | 未导出、未建模 | ◐ 信息缺 | P0 |
| G12 | GETWORK 路整件重画、不分属性 | 我们比 core 细 | ⚪ 无 core 对照物 → 只能靠两态门验 | 纪律 |

---

## 7. 下一步完善增量更新的计划

原则不变：**「少算」方向的每一步都要两票**（core 的字典 / 规则 + 我们自己的读取集合或两态对拍），「多算」方向随时可做。

### P0　口径与数据（0.5 天，纯文档 + 一个导出脚本）　★0915 S1 落地：1–6 全部完成（见 `changelog.md` 2026-09-15 与开发计划 §4 S1）

1. `docs/specs/core3d-partial-update-conformance.md` R5 加注：「没有 TransformOnly」只对 DRAFT 侧成立，3D 侧 `UPGRPH` 类别 3 = `STRTRA`/`SEGTRA`（本文 §4）。
2. `docs/evidence/…/2026-09-06-ida-core3d-change-evalat-cascade.md` §1 `IDCHNG` 行改正：INTUBE 旗标读 `PLCF`（652066），不是 `DCHC == 1`。
3. `上下文/…-800a23b3.md` §4「`changesBetweenSessions` 完全不看 CACHID」改正：`DDES_GraphicsMngr` 那条路不看；`DES_DrawListManager::changesBetweenSessions` 的 `DES_DrawListCompare::attChange` **只**看 CACHID。
4. `CONTEXT.md`「模型影响 (Model Impact)」词条补一句：三档的 core 对应物（类别 0 / 类别 3 `STRTRA`·`SEGTRA` / 类别 4 `REPELE`·`SEGREP`）与「GETWORK 路 core 不分属性」。
5. `output/gen_dchc_fixture.py` 同时提炼 `PLCF`（652066）与 DCHC；`raw_dchc_code` 旁加 `raw_plcf(name)`（只作报表 / 探针用，不进判决）。
6. `model_impact.rs`：把 DCHC=4 而五张表未登记的 213 个名字**生成**进第六张表 `DICTIONARY_GEOMETRY_ATTR_NAMES`（效果 `DirectGeometry`，来源字典），
   消掉回执里的 `Unknown` 噪音；行为不变（守卫：两态计划逐字相同）。

### P1　刚体前移开门前置：把 `UPGRPH` 类别 3 的例外补进 `rigid_anchor`（1–2 天，`window_root_plan.rs` + `rigid_motion.rs` + e3d-model `category`）

- **P1-a（G1）** 排除 P23：候选 noun ∈ 那 23 个 → `Other("placement-derived noun")`，根进 `regen`。名单以常量入 e3d-model `category::PLACEMENT_DERIVED_NOUNS`，
  注释带地址 `0x1022EAEE`；测试钉「恰好 23 个、逐个点名」。
- **P1-b（G2）** 排除「被属主消费的几何」：锚点 `!is_model_unit(el)` 且不是纯容器（`classify` 为 `List`/`Composite` 之外的非单元：负体 `N*`、`PLOPEN`/`WLOPEN`/`BPOPEN`、`FIXING`/`TMPL` 开孔、剖面 / 点成员）→ `regen`；
  另按规则 M：锚点的 T 端祖先含 `PANE`/`GENSEC`/`SCTN`/`BPANEL` 且锚点不是独立单元 → `regen`。
  这是 ADR-066 D1 规则 3 的补全（原文只列了路由容器 / 点容器 / CWBRAN 三条）；落 ADR-066 修订注。
- **P1-c** 对拍门补样本：ams8000 链上找「只挪一个 `NBOX` / `PLOPEN` / `HRPOST` 的窗」（`probe_*` 探针同款，`#[ignore]`），无自然窗则按 ADR-066 验收 1 的 `AIOS_TRANSFORM_WINDOW` 造一窗；
  两态逐条相等后再翻 `model_transform_cheap_path` 默认值。
  ★0916 S4 离线半程已落地（`window_root_plan::tests::probe_rigid_exclusion_windows`，`docs/evidence/2026-09-16-rigid-exclusion-parity.md`）：全语料 710 个只动摆放的候选里
  合格锚点 27 个有判别力的根两态全等（0 红）；负体（NCYL → 属主 FLOOR）、规则 M（FIXING / POINSP → 属主 GENSEC）、路由容器（VALV / ELBO / GASK → 隐式管身）三条排除各有
  干净样本坐实「照 Δ 前移就错」；`NBOX` / `PLOPEN` / `HRPOST` 与 CWBRAN / 点环容器 / 桥架件 / 剖面 / 几何集在语料里**没有**自然窗，造窗那一半还在。
  剔掉 `CACHID` 再判多出的 84 个锚点（整台 EQUI 挪位）108 根全等——开发计划 §8 Q5 的第二票。
- **P1-d（可选）** `BFORI` 进 `TRANSFORM_ONLY`：先查它挂在哪个 noun、我们是否生成；不生成就只记文档。

### P2　「不影响模型」从 5 个名字扩到「读取集合之外」（2–3 天）

1. ★0915 S5 已落地（`output/gen_read_set.py` → e3d-model `data/attribute-read-set.json` 133 名 + gen-model `generation_read_set.json` 并集 138 名；
   DCHC=0 恰好 `DIR FILL GTYP PKEY UKEY VXREF` 六个；e3d-io 目录求值环境的读点单列不投票；两仓护栏各一组）。原文：
   **生成读取集合成为受护栏的产物**：把 e3d-model `tests/reference_reads_guard.rs` 的思路推广到**全部**属性读点——
   扫 e3d-model `src`（本轮一次性探针得 131 个运行库属性：DCHC 4 ×108 / 3 ×2 / 1·2 ×7 / 0 ×6 / 字典无 ×8）+ gen-model 侧模型面读点（`cata_model` / 房间 / `structural_connection` / `cable_endpoint`），
   导出 `generation_read_set.json`（含间接读点登记，同 `INDIRECT_READ_SITES` 纪律），守卫测试钉「源码里的每个属性字面量都在集合里」。
2. ★0915 S6 已落地。**新判据**（替换 5 名单，仍是两票）：`change_metadata_tier(diff)`（`change_is_metadata_only` 退成布尔包装）= 四条结构条件 ∧ 每个变更属性
   **(DCHC = 0 ∧ ∉ 读取集合)**（`model_impact::attribute_is_metadata_only`）。UDA、字典没收的名字、`opaque` 照旧重算。`DIR FILL GTYP PKEY UKEY VXREF` 由读取集合那一票自动挡住。
   落地时多出一条原文没写的：五张手写表里 **16 个**名字（`HREF TREF PSPE DKEY DDPR FSPREF SCREF STYP CURD JFRE PTYP` + `SIZE MAXA NGMR ORRF PTOF`）字典 DCHC=0 且
   生成器不读——§1.4 那句「13 + 6 个是我们的几何输入」被 S5 的扫描推翻了一半（真读的只有 `GTYP PKEY VXREF`）。判决以两票为准（只改它们 → Skip），表项不动，
   守卫逐个点名（开发计划 D2 ★S6 (1)）。
3. ★0915 S6 已落地（测试与探针）。**两态门**：三条契约抽成 `assert_metadata_gate_two_state`，`live_ams8000_metadata_only_gate_never_adds_and_never_shrinks_the_closure`
   照跑；全语料版 `live_corpus_metadata_only_gate_two_state_over_every_desi_db` 沿五个项目目录的每个 DESI 库整条会话链跑：新门放行的窗只能多不能少、账本守恒、
   **闭包只少不多**（★0916 改口：原写「闭包不缩」，全语料门在 aps250514 3→4 撞出 CWBRAN 只改 `HCREF`/`TCREF` 的窗——关着时桥架分支粗级联整条重建 20 个 CTRAY、
   闭包多 42 个 refno，开着时两票放行、一个都不重建；契约改成「账本种子两态相同 ∧ 少掉的 refno 全是少重建的单元或其祖先」，见开发计划 D2 ★S6 (5)）；
   顺带出「语料里 DCHC=0 属性变化的频次表」（`docs/evidence/2026-09-15-metadata-gate-two-vote-corpus.md`）——哪些改法在真实数据里常见，决定这一步省多少。
4. ★0915 S6 已落地。回执：`no_model` 里区分 `metadata_only`（5 名单）与 `unread_attribute`（新档）（e3d-model `NoModelTier` / `IncrementReport::unread_attribute`，
   `totals_line` 多一格），预览与执行同一谓词（0908 那条纪律不破：`classify_operation_effects` 逐属性先问门，`change_impact` 的 `action` 由两票说并多 `dchc` / `generator_reads` 两格）。

### P3　图元级重建（core 的 `SEGREP` 粒度）——先量再定（探索，1 天量 + 视结果立 ADR）

- 今天嵌套根 / 重复覆盖已有指标（`nested_roots` / `duplicate_coverage_elements`、`last_root_duration_ms`）。补一条：**每窗被重算的根里，有几件单元的记录与上一版逐字相同**（manifest 逐条比），
  就是 core 用 `SEGREP` 省下、我们白算的量。
- 若单根重算里 >70% 的单元记录不变（大 EQUI 改一个 BOX 的形状），立 ADR：单元级发布需要单元级 CAS / 回执 / scoped delete（ADR-056 D3 的例外，`increment.rs` 模块文档已写明这一步不许绕）。
- 否则维持根级，记结论。

### P4　`CACHID` 一致性普查（0.5–1 天探针，不进生产）　★0916 S8 已跑完（会话 fable-5-1-49）：结论在 §4.4 末条与 `docs/evidence/2026-09-16-cachid-consistency-corpus.md`——「有 DCHC≠0 变 ⇒ CACHID 变」在有值元素上 98.9% 成立、反向只有 2.7%、92.8% 的跳动是整库扫写形状；只能作单向离线校验，不作判据，不接线

- 沿 486 库会话链：每个 Modified 候选上 `CACHID` 变不变 × 变更属性的 DCHC 类别 × 我方判定（regen / transform / skip）三维交叉。
- 若「有 DCHC≠0 属性变 ⇒ CACHID 变」在语料上成立，`CACHID` 可作我方分类器的**离线校验 oracle**（不是判据：写它的一方不在 core，缓存模式关着时不一定写）。
- 顺带答 §4.4 的开放问题：哪些 noun 上真有值、值的形状（"1" 是计数还是旗标）。

### 排期与依赖

P0 独立、随时可做；P1 是开 `model_transform_cheap_path` 的前置（G2 是正确性缺口，先于性能收益）；P2 依赖 P0-5/6 的导出与 P1 无冲突；P3、P4 独立。
建议顺序 **P0 → P1 → P2 → P4 → P3**。与在途计划的关系：不与 `2026-09-15-increment-task-durable-boundary-dev-plan.md`（任务边界）、
`2026-09-15-cata-dependency-closure-cache-dev-plan.md`（CATA 闭包）撞文件；P1 改的是 `window_root_plan.rs` / `rigid_motion.rs` / e3d-model `category.rs`，P2 改 `model_impact.rs` / `window_root_plan.rs` / e3d-model `increment.rs`。

---

## 8. 不许当已证

- ~~`UPGRPH`（`0x1022EAEE`）、~~`TRASCN`、`STRTRA`、`SEGTRA`、`SADREP`、`REPELE`、~~`IDCHNG`、~~`EVALST` 是 Hex-Rays 伪码 + `get_dword` 实读常量：调用序、埋点名、常量已核，
  **分支结构未逐指令核**。它们是 Fortran 风格平铺调用（`DGETI`/`DFIND`/`GLSMAT` 走导入表），没有核对表 §1.4 那类 `getField` 出参陷阱，伪码采信；但 `TRASCN` 里 `DDEST(-1)` 的语义
  （推定为「`DFIND NEXT` 自动下钻、不限深度」，与循环里 `DRETUR` 回上层的形状一致）只对了名表（opcode 37 = `DDEST`），**未读体**。
  ★0916 **`UPGRPH` 与 `IDCHNG` 已逐指令核**（§9.4 / §9.5）：`UPGRPH` 的 `cmp eax,3` / `cmp eax,4` 分派、P23 那 23 个 `cmp eax, imm32` 常量（逐个逆 hash，与 §4 名单同序同集）、
  `EQREF(top, ref)` / `LPRMTV` / `IPCOMP‖IHCOMP` 三级分叉与每一叉的调用目标全部对上 §4 伪码；`IDCHNG` 两次 `ATAINT` 与 `cmp eax,1 → dword_10E98500 = -1` 对上 §3.2。
  `TRASCN` / `STRTRA` / `SEGTRA` / `SADREP` / `REPELE` / `EVALST` 仍是伪码级。
- `hasTopLevelGraphicsChanged` 是逐指令读的（Hex-Rays 把 `attributesChangedBetween` 的出参丢了，伪码恒 `return 0`，不可用）。
- `DES_DrawListCompare` 十二个回调读的是伪码，但都是 ≤ 10 行的小函数（空函数 / 一次 `set::insert`），可信。
- 「GETWORK 路不分属性」这句依赖 `DB_RawChanges` 只走 `DB_IndexTableCompare`（`createListBetweenSessions` 伪码）与 `sub_1052F2E0` 传 REDRAW（0906 逐指令）；
  `DB_RawChanges::trim(vector<DB_DB*>&, …)`（`0x5984650`）已读：对四个集合各套一个 `DB_Raw::PredicateDBNumMatch`——**只按库号裁**，没有属性判据；
  `trim(set<DB_Element>&)` / `trim(vector<DB_Element>&)` 两个按元素集合裁的重载没读体。
- `PLCF` 的语义只知道字段名（逆 hash）与「== 1 置 INTUBE 旗标」；~~字典里哪些属性 `PLCF == 1` **未导出**（P0-5）~~ ★0915 已导出（§3.2，58 个），
  ~~但「置了旗标之后 `PLCDEL` 到底清哪些缓存」未读体~~ ★0916 `PLCDEL` 已读（§9.6）：清的是 Core3D 里一张 100 槽、按 `(DB_Ref, int)` 键、存 3 个 double 的缓存表
  （`dword_10E7BCE4`），`VDESPT` 入口也无条件清一次；「p-line 缓存」是四条腿撑着的**推断**（§9.6 末段），字段名 `PLCF` 展开成什么没有字面证据——所以它仍只作报表，不进判决。
  另一个边角：`IDCHNG` 第二次 `ATAINT` 的出参与第一次共用一格且**不检错**（§9.5），`PLCF` 读失败时比的是残留的 DCHC 值——字典属性都有这一格，只在字典外属性上成立，而那些属性第一次 `ATAINT` 就失败早退了，实际不可达。
- `CACHID` 的写方不在本机任何 IDB 里。`D:\AVEVA\Everything3D2.10` 全目录（868 个 dll/exe）扫 `CACHID` 字节串 **0 命中，连 2.10 的 core.dll 里都没有这个属性名**
  （同一扫法 `ATT_HPOS` 命中 4 处，方法有效）——`CACHID`（与 `LCHKDA`）是 **3.1 才有的属性**，图形缓存路是 3.1 的新东西；而本机 3.1 只有 core.dll / libgeom / libgm 三个副本，
  Design 应用侧的 DLL 不在。§4.4 「写方在 Design 应用侧」是排除法推的，不是读出来的。
  ★0916 S8 从文件侧坐实了写方的**行为**（`+1` 计数、按应用分、成员增删跳属主、整库扫写），写方是谁仍未读出；「12476 个只 `CACHID` 且子树无变的
  跳动是缓存重建扫写」是从形状推的（同一会话里同 noun 全体 `+1`、同窗别无改动），没有读到写它的代码，也排除不了「被引用的目录件变了」这一种来源
  （探针只看 DESI 窗口，不看 CATA）。
- e3d-model 读取集合 131 个是**正则扫源码**（2–6 位大写字面量 ∩ 运行库属性名 − noun 名）的估算：漏掉 >6 位的长名与经变量传入的读点，也可能把同形常量算进来；P2-1 要用护栏测试把它变成准确的产物。
- 本文没有跑任何一条联动 / 分类的实测；§6 的 ✅ 是「规则形状同构」，不是本轮复验。
- ★0916 §9 的读法：`DESDRA_SCPlugs` 十二个钩子、`descases/*` 十一个例程、`DB_Element::internalPutAtt` / `postSetAttribute`、`sub_5992C80` 是 Hex-Rays 伪码 + `get_dword` 实读常量；
  **`VDESPT` 的 DCHC 门与 `PLOO.HEIG` 分支、`UPGRPH` 分派骨架、`IDCHNG` 全函数**是逐指令读的。`maplist` 只读了 `MLSADD` 与消费者 `VDEMPR` 两个体，「空间地图包围盒」是从
  `mapboxlib/MPBOX` / `MPBXBR` 的埋点名 + `WSTRAM` 调用推的（`MLSADD` 读的字段 `IMAP` 与快照 `spatial_map` 列的对应是实读常量 + 取值集合对上，`IMAP` 六个取值各自的语义仍是从 noun 成员推的）；`colsel/AUTCO2` 与 `designEvents` 只看了调用序；`sub_1023190B` 的模块码（`Q`→3、`S`→5、`_`→6、ESC→4、其它 1）
  没有对到模块名。`internalPutAtt` 的同值早退：2026-11-04 已读实的是标量 int（`0x59419d0`）、double（`0x5941ed0`）、bool（`0x5942370`）、元素引用（`0x593f220`）、字符串（`0x593f680`）、bool 数组（`0x59415a0`）、元素引用数组（`0x59410c0`），都是旧值相等就返回，不到 `dabPutAtt` / `postSetAttribute`。int 数组（`0x5940190`）和字符串数组（`0x5940e80`）的函数体没有旧值比较。double 数组（`0x5940480`）以及 `putPosAtt` / `putDirAtt`（各 46 字节，转到 `sub_58504E0`）也不比较旧值，插件回调未读。「先 `PostSetRefAttribute` 再写再 `postSetAttribute`」仍只在 ref 版上读过，其余重载的事件顺序未逐个展开。

## 9. ★0916 会话内交互路补全：从 `putAtt` 到 `VDESPT`，属性专项规则与 QCHGLS 的全部写入者

- 手段同 §1：ida-bridge 活桥，`core.dll` = `idalib-33060`、`Core3D.dll` = `idalib-49136`（E3D 3.1）。常量全部 `ida_bytes.get_dword` 实读后 `ENHAS2` 逆解。
- 起因：§2 的「会话内交互路」只画到 `descases/VDESPT` 这一格，没说 **C++ 的属性 put 是怎么落到它头上的**、`VDESPT` 除了 DCHC 门还做什么、以及 **QCHGLS 除了 `EVALAT` 还有谁写**。
  这三件事决定「一个属性 put 在 core 里到底触发什么」这句话说得完不完。

### 9.1 从 `DB_Element::putAtt` 到 `descases/VDESPT`：一条属性 put 的完整时序

core.dll 侧（`DB_Element::putAtt` 全部重载 `0x59461a0`–`0x5948a20`、`putAttWithinRuleEngine` `0x5949220`–`0x59495f0` 都只是壳，
真身是按值类型分的十个 `internalPutAtt` `0x593f220`–`0x5942370`；下面的时序按 ref 版 `0x593f220` 读。同值早退已在 §8 列出的七个重载上读实；int 数组、字符串数组、double 数组没有这条早退。其余步骤的顺序在非 ref 重载上没有逐个展开）：

```text
DB_Element::putAtt(attr, [qualifier,] value)
  └ internalPutAtt(attr, qualifier, value, evaluateRules = true)
      0. isDistributedAttribute(attr) → 改道到 XRQELE 成员元素上 putAtt（事件在持有元素上发，不在逻辑元素上）
      1. getAtt(旧值)；旧值相等 → return true（§8 列出的七个重载；int 数组、字符串数组、double 数组没有）
      2. DB_PreSetAttributePlugger::PreSet*Attribute                     可否决
      3. isUDA → isNounAllowed；updateAllowedWithClaim（claim）
      4. DB_LegalityChecksPlugger vtbl+28  Set*AttributeAllowed          可否决（DESDRA_SCPlugs 也实现了这一族 16 个 Allowed）
      5. [ref / ref-list 专有，写之前] DB_ElementChangesPlugger vtbl+60 PostSetRefAttribute(el, attr, target)
                                        / vtbl+64 PostSetRefListAttribute(el, attr, targets[])
      6. 写：DB_PseudoAttPlugger::put*Att（伪属性）| putUda | dabPutAtt
      7. DB_Element::postSetAttribute(attr, qualifier, evaluateRules)     0x59453b0
           受影响集 = isCloneable(attr) ? DB_Clone::getRelatedElements(el) : { el }     sub_5992C80 0x5992c80
           for e in 受影响集:
               DB_ElementChangesPlugger vtbl+52  PostSetAttribute(e, attr, qualifier)    ← 主事件
               attr == ATT_NAME → vtbl+68 PostSetName(e)
               attr 是 UDA → DB_Uda::GetControlledUdas：受控 UDA 逐个复验，不合法的 clearUda
               evaluateRules → DB_Element::evaluateRules(e, attr)         ← 规则引擎，可能再 put 别的属性 → 递归本链
```

`??_7DB_ElementChangesPlugger@@6B@ = 0x5d19234`：+52 `PostSetAttribute(el, attr, qual)` `0x591e5b0`、+56 两参版 `0x591e530`（只是构造空 qualifier 转 +52）、
+60 `PostSetRefAttribute` `0x591e720`、+64 `PostSetRefListAttribute` `0x591e780`、+68 `PostSetName` `0x591e6c0`。广播体都是「过 `DB_Attribute::wnoevt(attr)` 闸门 → 逐订阅者虚调用」
（0007 记录读过 RefList 版，本轮读 Ref 版同款）。**`wnoevt` 为真的属性不发任何变更事件**——这是 DCHC 之前的又一道门。

★0917 `wnoevt` 的来源读通了：`DB_Attribute::wnoevt`（`core.dll 0x58D5290`）只是 `return this[+184]`，那一格由
虚表 +20 的 `DB_Attribute::ReadData`（`0x58CE1F0`）从字典字段 **`WNOEVT`（`299311034`）** 读入（`sub_55BB6C3` 逻辑字段读，
出参 `& 1`）；紧挨着的 `+185` 来自 `WNOCLM`（`193514909`）。同一个 `ReadData` 里 `DCHC`（`596407`）落 `+92`，
所以 **`WNOEVT` 与 `DCHC` 是同一条字典记录上的两格**。此前 ADR-008 导出器把 `+185` 的 `WNOCLM` 导成了
`noun_layout.json` 的 `noClaim`（191 个为真），**唯独跳过了 `+184`**。

★0917-b 导出器已补上（`scripts/e3d/NounLayoutExport.cs` 的 `UnnamedFields`）：`DbAttributeField` 枚举漏了
`WNOEVT`/`WNOCLM`，但**它的成员值就是 PDMS 字段选择器 hash**（`DCHC=596407`、`PLCF=652066`、`CASC=604371`… 全对得上，
反编译 `Aveva.Core.Database.dll` 的枚举实证），而 `DbAttributeImpl.GetInteger` 把 `(int)field` 直接转给
`DB_Attribute::getField`——所以把 `299311034`/`193514909` 强转成 `DbAttributeField` 再 `GetInteger` 就能读到，和 `DCHC`
一条路。重编 x86 通过（此 3.1 版还需加 `Aveva.Core.Geometry.dll` 引用）。**但本轮无人值守跑不出数据**：`des.exe -tty` 下
PML.NET 建对象根本不成（连已知能用的 `object RVMEXPORT()` 也 `import` 成功、建对象失败，是 `-tty`/PMLNet 桥的问题），
detached/隐藏 GUI 会话在第一个 idle 前就退出。于是改走**纯离线**，见下。

★0917-c **`WNOEVT`/`WNOCLM` 已从 `attlib.dat` 纯离线解出，不再依赖活 E3D。** 把读取链逆到底
（ida-bridge，`core.dll = idalib-33060`）：`ReadData` 的每个字段都走 `sub_55BB6C3`/`sub_55BB60C`（PDMS **ATNLOG**），
`value = pool[512*page + base + header[ordinal]]`；`ATTOPE`（`sub_55F4290`）证实 `pool`（`dword_6C3F6C0`）与它的索引就是
`attlib.dat` 的 **ATGTDF**（`dir[0]` → 有序字段表 `dword_6BE26E0` + 计数 `dword_6C08EB4`=63，给出字段序号）与 **ATGTIX**
（`dir[2]` → 属性索引，`(hash, packed=page*512+word_offset)`）——正是 `vendor/e3d-attlib` 已经在解的那套。记录布局：一行
= `[N 个字段偏移头][值]`，`FHDBRN` 块号 1-indexed，故记录落在文件 `(page-1)` 页、`base` 字处：

```
row   = (page-1)*512 + base - 2          # 0-based file words（大端）
off   = word[row + ordinal]              # 头槽：-1=用 schema 默认(0)，0=继承(0)
value = word[row + off]
```

生成器 `output/gen_wnoevt_flags.py` 实现了它并**自校准**：解 `DCHC`（序号 49）对齐 `dchc_change_classes.json`
**4270/4270 逐条精确**、解 `PLCF`（序号 50）对齐 `plcf_flags.json` **58/58**——对不上就拒绝出数（防 attlib 版本漂）。
结论：全字典 **只有 7 个属性置了 `WNOEVT`**（`DPOSCA EVITCA EVITCP EXTEMP LIGTCA LIGTCP TEMPLI`）、6 个置 `WNOCLM`
（`EXTEMP HANOFF HANPOS LIGTCA LIGTCP TEMPLI`），**全是激光/照明/温度属性，没有一个在模型生成的属性集里**。也就是说
增量判决看到的每个属性 `WNOEVT` 都为 0，这道门永不触发。产物 `src/data_interface/wnoevt_flags.json` 随源码入库，
`attribute-verdict-explorer.html` 的 `WNOEVT` 面板已换成这个权威结论、命中这 7 个之一时行上打 `WNOEVT` 徽章。
（`WNOCLM` 与 ADR-008 导出器的 `noClaim` = `DB_Attribute::noclaim(attr, elementType)` 不是一回事：后者按元素类型算，58 个 noun 级为真；前者是纯字典字段，只 6 个。）

★0917-d 同一套离线解码**落成了 Rust**（`vendor/e3d-attlib/src/dict.rs` 的 `e3d_attlib::dict::AttrDict`，忠实复刻
`sub_55BB6C3`：字段序号取自系统 ATGTDF、记录 `(page,base)` 取自**系统** ATGTIX `dir[2]`、`value=word[row+header[ordinal]]`，
`-1`→schema 默认、`0`→SYNO 继承、UDA→只给默认）。带可移植校准测试 `tests/dict_calibration.rs` 与 `cargo run --example
wnoevt -- <attlib.dat>`；对影子 `attlib.dat` 跑出 WNOEVT=7 / WNOCLM=6，与 Python 生成器逐字一致。**注意 `dir[2]`（系统属性
索引）与 `dir[6]`（元素类型/名词索引：`SBOX TUBE LINE`… 82 个元素类型也带 `WNOEVT`）是两个命名空间**——逐属性门只算
`dir[2]`，故是 7；一度量出的「89」是把两者混数所致。

★0917-e **`WNOEVT` 事件门已接进增量判决**（用户 2026-09-17 拍板）：`model_impact::attribute_no_event` 在
`attribute_is_metadata_only` 首行、**DCHC 两票之前**把置位属性无条件短路为「不动模型」（对齐 core `putAtt` 里 `wnoevt`
先于 `DCHC` 的门序）。预览与 direct 窗口规划器共用这一道谓词，故同口径。因这 7 个都不在模型 schema / 读取集合里，对
现有语料是**恒等变换（回执基线不变）**，只在它们万一出现时把「未知→保守 Regen」纠正为「不动模型」；
`model_impact::tests::wnoevt_gate_is_disjoint_from_reads_and_geometry` 钉住这条安全不变量（WNOEVT ∩ 读取集合 = ∅、且均无正 DCHC 码）。

Core3D 侧，**订阅者就是 `DESDRA_SCPlugs`**（单例 `Instance` `0x10409440`；`Init` `0x10409160` 一次订阅 `DB_LegalityChecksPlugger` 16 项、
`DB_ElementChangesPlugger` 12 项、`DB_ProjectEventsPlugger::PostOpenProject`、`DB_MDBPlugger::addPlug`）。每个 Post 钩子只按**库类型**分流：
`DB_DB::type == 7`（PADD / DRAFT）→ `dra_scase/*`，其它一律 → `descases/*`。**没有 DESI 专属门**——CATA 元素的属性 put 也进 `VDESPT` → `EVALAT`
（这正是会话内改 `SPCO` / `SVER` / `DATA` 走 `UPGRPH` 类别 4 `REPALL` 全部重画的入口，§4.2）；GETWORK 路那道 DESI / ISOD 门在 `sub_1052F2E0`，核对表 R1 不受影响。

| `DESDRA_SCPlugs` 钩子 | 地址 | 非 DRAFT 去向（`descases/*`） | 推进 QCHGLS 的方式 | 其它副作用 |
|---|---|---|---|---|
| `PostSetAttribute` | `0x10409A60` | **`VDESPT`** `0x101F33A9`（唯一调用方） | **DCHC ≠ 0 才 `EVALAT`**（§9.2） | 四条属性专项规则、`MLSADD`、自动着色、`designEvents` |
| `PostSetRefAttribute` | `0x10409BE0` | `VDESPF` `0x101F2A27` | **不推** | `structures/BAKREF` `0x102D4724` 维护 back-ref；`SPCO.PRTREF` 特例（§9.7） |
| `PostSetRefListAttribute` | `0x10409C50` | `VDESFA` `0x101F2D4B` | **不推** | 新旧列表差分 → `BAKREF` / `structures/BREAKF` `0x102D448F`；`COCO.COCORE` → `CTYP` 同步；`PSARFA` `PSLRFA` `ALERFA` 三个不建 back-ref |
| `PostSetName` | `0x10409B30` | `VDESNA` `0x101F3BF2` → `change/DEEVNA` `0x1022E275` | **不推** | `UQGRAF` 时 `aids/RMKELE`（只刷标注 / 辅助图元）+ 置 `dword_10E985E0` |
| `PostCreateElement` | `0x10409710` | `VDESCR` `0x101F1290` | 模块码 ≠ 3 时 `change/EVALNW` → `EVALAT(REDRAW)` | `INGCOM(noun) == 1`（负几何件）且 noun ≠ `NGMS` 先 `BPUTI` 一个缺省值；`MLSADD`；`sub_105016E0` 事件 |
| `PostIncludeElement` | `0x104097C0` | `VDESIR` `0x101F2755` | `EVALNW`（新位置整件） | `MLSADD` |
| （include 的删旧端） | `sub_101F23F4` = `VDESI1` | `SCLREM` + `sub_105683C0` + **`SCINC2`** `0x101F2638` | `EVALAT(INTUBE)` 再 `EVALAT(REDRAW)`（0906 §2.2b） | `maplist/MPDEL` |
| `PostCopyAllElement` | `0x10409610` | `VDESAC` `0x101F1562` | **直接 `EVALCD(el, 4)`，不经 `EVALAT` 规则表** | `PLCDEL`、`GTPINI`、`MLSADD` |
| `PreDeleteElement` | `0x10409DD0` | CATA（type 2）→ `CDELET`；其它 → `VDESDL` `0x101F1B24` | `EVALAT(INTUBE)` 再 `EVALAT(REDRAW)` | 失败信息 `CopyFromFortran` |
| `PreReorderElement` | `0x10409F10` | `VDESR1` `0x101F27F2` | `UQGRAF` 时：owner 是 `BRAN` → **前一成员** `EVALCD(prev, 4)`（无前一成员则 `BRAN` 自身）；owner 是 `FLRCOV` → `EVALCD(owner, 4)` | `maplist/MPDEL` |
| `PostReorderElement` | `0x10409920` | `VDESRO` `0x101F2989` | `EVALCD(el, 4)` | `MLSADD` |
| `PreIncludeElement` / `PreCopyAllElement` / `PostCopyAllElementMembers` | `0x10409EB0` / `0x10409D60` / `0x10409690` | 未读体 | — | — |

`EVALAT` 的调用方就此闭合（`xrefs → 0x1022C679` 共 8 处 5 个函数）：`VDESDL` ×2、`SCINC2` ×2、`VDESPT` ×2、`EVALNW` ×1、`sub_1052F2E0`（GETWORK）×1。
`EVALAT` 入口门 `UQGRAF() ‖ sub_1050DCA0()` 的第二项已读：`sub_1050DCA0` = `DES_DrawListManager::instance()` 字节 `+9` ≠ 0，就是 §1.2 的 `cacheGetworkChanges` 旗标——
**图形关着但缓存模式开着，变更也照样进 QCHGLS**。

### 9.2 `descases/VDESPT(el, noun, attr)`（`0x101F33A9`）：DCHC 门之外的四条属性专项规则

DCHC 门与 `PLOO.HEIG` 分支是逐指令读的（`0x101f3419`–`0x101f3510`），其余伪码 + 常量。常量表 `unk_10B200D8`：
`596407 DCHC · 10206636 OWNER · 642215 TYPE · 653673 CREF · 643150 JFRE · 855256 DELP · 865137 CTYP · 82139754 COCORE · 275899206 COCDES · 209154153 COCONN`；
内联字面量 `675926 HEIG · 837961 PLOO · 719812 SJOI · 829281 COCO · 840256 PPRO · 840244 DPRO · 565735 DATA`。

```text
VDESPT(el, noun, attr):
    descases/SCSPOL(noun, attr)                       模块码 83('S') 下：按 noun 的 DBTP 表逐项调 sub_106F4D70（未读）；SPLDRG.SFLIMI 另调一次
    PLCDEL()                                          ★ 无条件清缓存表（§9.6）——不看 DCHC、不看 PLCF
    catdblib/GTPINI()                                 目录 p-point 缓存重置
    (dchc, err) = ATAINT(attr, DCHC)                  0x101f343b
    if dchc != 0:                                     0x101f3449 test eax,eax / 0x101f344b jz → 跳过整段
        if attr == HEIG and noun == PLOO:             0x101f3456 cmp 0A5056h / 0x101f3466 cmp 0CC949h
            owner = DGETF(OWNER); DGOTO(owner); t = DGETI(TYPE)
            EVALAT(owner, t, HEIG)                    ★ 规则 U：PLOO 的 HEIG 当作 owner（PANE / FLOOR）的 HEIG 评估
            DGOTO(el)
        else:
            EVALAT(el, noun, attr)                    §2 / 0906 §2.2 的规则表
        maplist/MLSADD(el, noun, 0)                   ★ 进空间地图包围盒清单（§9.3）
    if attr == JFRE and noun == SJOI:                 ★ 规则 V（写回，不进队列）
        cref = DGETF(CREF); jfre = DGETL(JFRE)
        if cref 非空 and jfre:                        节点刚被置成「自由」
            org = pllib/GJDORG(el)                    暂 DPUTL(JFRE, false) 后取 pllib/PICOPS 位置与 pllib/GJTOR0 矩阵
            DELP += GJTOR0 · (PICOPS − org)           DGETRA/DPUTRA(DELP, 3)，错误 18 清掉
            DPUTL(JFRE, true); PLCDEL()               ← 让节点留在原地：JFRE 翻真时把位置差折进 DELP
    if attr == CTYP and noun == COCO:                 ★ 规则 W（目录侧写回）
        ctyp[2] = DGETIA(CTYP); refs[2] = DGETFA(COCORE)
        for i: refs[i] 非空 且 GATWR1(refs[i], COCDES, COCONN) != ctyp[i] → NULIFY(refs[i])
        改过 → DPUTFA(COCORE)                         连接类型改了，不再匹配的 COCORE 引用置空
    if attr in {PPRO, DPRO} and noun == DATA:         ★ 规则 X：descases/SCRUSE(el)（DATA 元素的属性 / 缺省属性变了 → 规则重用侧）
    if 全局开关 unk_10FC3B30 & unk_10FC5A78:          ★ 自动着色（colsel/AUTCO2）：属性变了可能换画列表
        if add/LADDED(el, &old) and AUTCO2(el, 2, &new) and old != new:
            add/ADDREM(el, old, add/REMELE); add/ADDREM(el, new, add/ADDELE)
    sub_10501A20(el, attr)                            designEvents::instance_->vfn[1](1, "=<w0>/<w1>", <attr 名>)：C++ 侧「属性变了」事件，消费者不在本 IDB
```

对本文主题的四点：

1. **DCHC=0 的属性在 `VDESPT` 就被挡了**——比 §3.3「`EVALAT` 入口 `if (v16)`」还早一层，且这一层是逐指令核的：R13 上游的「上游」。但 `PLCDEL` / `GTPINI` / `SCSPOL` /
   自动着色 / `designEvents` 都在门外——**DCHC=0 不等于 core 什么都不做**，只等于不动几何。
2. **规则 U 是 §2 规则表（A–T）之外的第 21 条 noun×属性专项规则**，而且它在 `EVALAT` 之前改写了入参：`PLOO.HEIG` 进 `EVALAT` 时元素已经是 owner。
   我方：`PLOO` 是 `is_loop_container_noun`（`POPF` 位）成员容器，任何属性变化都归属主重算（`resolve_element_generation_root` 跨 loop 容器上溯）——**同构、更粗**；`HEIG` 在 `DIRECT_GEOMETRY` ✅。
3. **规则 V / W 与 0906 §8.2 的 `RECON0` 同一族：core 在属性 put 的钩子里再写别的属性。** 文件差分看到的是**结果**（同一窗里 `JFRE` 与 `DELP` 一起变、`CTYP` 与 `COCORE` 一起变），
   我们不需要也不该模拟这些写；它们解释了语料里「一次操作多属性同窗变化」的一部分形状。`SPCO.PRTREF` 拷贝七个属性（§9.7）同理。
4. **`MLSADD` 与 DCHC 门在同一个 `if` 里**：core 只在 DCHC ≠ 0 时更新元素的空间地图包围盒。这给我方「Skip 档不更新 AABB / 空间树」一票 core 侧的佐证（§9.8）。

### 9.3 QCHGLS 的全部写入者（`xrefs → change/EVALCD 0x1022E020`，21 处 9 个函数）

| 写入者 | 埋点名 / 符号 | 触发源 | 推进的条目 |
|---|---|---|---|
| `EVALAT` 内部 11 处 | `change/EVALAT` | §9.1 的五个入口 | 规则表 A–T 的目标，类别按 §3.3 |
| `EVALRF` ×2 | `change/EVALRF` `0x1022DCAC` | `ADSTCH` 结构连接双向核对命中 | 对端 / 顶层件 |
| `VDESAC` | `descases/VDESAC` | `PostCopyAllElement` | `(el, 4)` |
| `VDESRO` | `descases/VDESRO` | `PostReorderElement` | `(el, 4)` |
| `VDESR1` ×2 | `descases/VDESR1` | `PreReorderElement` | owner 是 `BRAN` → `(前一成员, 4)`；`FLRCOV` → `(owner, 4)` |
| `ATTABK` ×2 | `descat/ATTABK` `0x101F0DEE` | **命令处理器**（`IGSPBK` 选项 2/3；4 = 当前画列表全部；5 = 指定 `BRAN`，否则错误 177） | 宿主里每个 `ATTA` 成员 `MLSADD(ATTA.REF)` 后 `(宿主, 4)` |
| `RACKO` | `rac/RACKO` `0x103196F4` | **命令处理器**（分支排序：日志串 `"Branch ordered"`） | `(BRAN, 4)` |
| `UpdateChangeList` | `DES_DrawListManager::UpdateChangeList` `0x1052B3C0` | `UPDATN` 消费队列时去重后按类别重灌（§8.5） | 原条目 |

即：**属性触发的 QCHGLS 写入只有 `VDESPT → EVALAT` 一条**；其余写入者是结构变化（建 / 删 / 挂 / 排序 / 拷贝）与两个命令处理器。
`VDESAC` / `VDESRO` / `VDESR1` 直接 `EVALCD`，**绕过 `EVALAT` 规则表**——拷贝或排序一个元件，core 不做上卷 / 联动，只把元素（或前一成员）自己记类别 4，
交给 `UPGRPH` 按 `FNDTOP` 折到顶层件重建。

`maplist`（`MLSADD` `0x10220F82` → `GLSADD` `0x10221257` 写、`VDEMPR` `0x102208C7` 经 `MLSNXT` 消费）：`MLSADD` 先按 noun 的字典整数字段
**`IMAP`（`847458`，`0x10221051` / `0x102210f9` / `0x10221198` 三处 `ATNINT` 的常量 `unk_10B22B80` 实读）**路由：0 → 不进；3 → 沿 `OWNER` 上爬到非 3；
7 → 取 owner（owner 是 2 除外）；`GROU` `GPWL` `WORL` 直接跳过；同一元素连续两次只记一次。`VDEMPR` 对每条：标志 1 → `mapboxlib/MPBOX`；
`BRAN` 或 `HTCOMP(noun)` → `WSTRAM` 后 `mapboxlib/MPBXBR`；其它再按 `IMAP` 决定要不要算。
**这是 core 的空间地图（`MAP`）包围盒维护清单**，与我方生成后就地收敛空间树 / AABB 是同一件事；写它的条件是 DCHC ≠ 0（§9.2）或结构变化。
`IMAP` 就是我方字典快照 `resource/dict/noun_caps.json` 的 **`spatial_map`** 列（`noun_dict.rs:68`）——取值 `{0: 1404, 1: 239, 2: 188, 3: 39, 5: 60, 7: 1}`
与 `MLSADD` 比的 `3` / `7` / `2` 三个常量对上：3 = `LOOP` `PLOO` `PAVE` 这类点 / 环容器（上爬到 `PANE`=1）、2 = `BRAN` `EQUI` `HANG` `SITE` `ZONE` `GROU` `WORL` 一类容器、
1 = `ELBO` `TUBI` `ATTA` `NOZZ` `PANE` `FLOOR`、5 = `BOX` `SCTN` `GENSEC` `SJOI`。0906 §2.2a「`847458 = IMAP` 本轮无消费者」到此有了消费者。

### 9.4 `change/UPGRPH`（`0x1022EAEE`，`0xf77` 字节）分派骨架逐指令核 —— §4 伪码成立

只取 `cmp` / `j*` / `test` / `call` 四类指令（312 条）重走一遍，与 §4 逐叉对照：

| §4 的叉 | 指令 | 目标 |
|---|---|---|
| `FNDTOP(ref, flag = noun == TUBI)` | `0x1022ecde cmp eax, 0AD7E9h`（`TUBI`）→ `0x1022ed1a call FNDTOP` | — |
| 「同一顶层件只处理一次」 | `0x1022ed32` / `0x1022ed7b EQREF` → `0x1022edcd jmp loc_1022EBE8`（下一条） | — |
| `LADDED` + 刷新观察者 | `0x1022ee59 LADDED` → `0x1022ee8e sub_1050D2E0`（`beforeChanged`）→ `0x1022eed9 ADDREM` | — |
| **`cls == 3`** | `0x1022eee7 cmp eax, 3` / `jnz loc_1022F44E` | — |
| P23 | `0x1022eef6`–`0x1022f050` **23 条** `cmp eax, imm32; jz loc_1022F05B`，逆 hash 依次 = `DRAW CLNTIL STRFLT INFITT PLTFRM ELFITT STRLNG HVACFI RAIL DOOR WINDOW FPFITT LINDIM FLRCOV MLABEL FIXTUR HRPOST HRTERM HRGATE HRPANE KICKPL CTSUPP FURNIT`（与 §4 名单同集同序） | `0x1022f092 ADDREM`（`REPELE`）+ `0x1022f0f8`–`0x1022f27b` 两层 `DFIND`/`DRETUR` 成员循环各 `RMKELE` |
| `top == ref` | `0x1022f290 EQREF` | `0x1022f2a2 TRASCN` → `0x1022f2c5 SEGOPN` → `0x1022f2db` / `0x1022f2ef STRTRA` |
| `LPRMTV(noun)` | `0x1022f306` | `0x1022f331 SEGTRA` → `0x1022f343 IPCOMP` / `0x1022f367 IHCOMP` → `0x1022f37b CRETUR` / `0x1022f38b NATTA` / `0x1022f3a4 DGETF` → `0x1022f3df` / `0x1022f40e SADREP` ×2 |
| 其余（容器） | `0x1022f444 TRASCN` | — |
| **`cls == 4`** | `0x1022f454 cmp eax, 4` / `jnz loc_1022F7F8`（→ 直接收尾，**类别 1 / 2 就是走这条**） | — |
| `REPALL` 条件 | `0x1022f467 ICCOMP` `0x1022f47f IGMCOM` `0x1022f497 INGCOM` `0x1022f4af IG2COM` `0x1022f4c7 IPLCOM` `0x1022f4df IPTCOM` + `0x1022f4fd cmp 0CA78Ch`（`SPCO`）`0x1022f53c cmp 0D9489h`（`SVER`）`0x1022f549 cmp 8A1E7h`（`DATA`） | `0x1022f550 REPALL` |
| `top == ref ‖ topNoun == TMPL` | `0x1022f56a EQREF` ‖ `0x1022f588 cmp 0BE598h`（`TMPL`） | `0x1022f5ca ADDREM` + `0x1022f60f TRASCN` + `0x1022f624 SEGOPN` / `0x1022f63e STRTRA` |
| `LPRMTV` | `0x1022f655` → `0x1022f689 IPCOMP` / `0x1022f6b3 IHCOMP` | 管件 `SADREP` ×3（`0x1022f72b` `0x1022f75a` `0x1022f789`），否则 ×1（`0x1022f7ba`） |
| 其余 | `0x1022f7f3 TRASCN` | — |
| 收尾 | `0x1022f815 COLRES` → `0x1022f83e RMKELE` + 两层成员循环（`0x1022f867`–`0x1022f9e7`） | — |

§8 里「`UPGRPH` 分支结构未逐指令核」这条结案；P23 的 23 个 noun 不多不少。

### 9.5 `change/IDCHNG`（`0x1022E302`，`0xe5` 字节）全函数逐指令 —— §3.2 成立，外加一个边角

```
0x1022e35b call ATAINT(attr, [0x10B23D70] = 596407 DCHC, &var_10, &var_C)
0x1022e363 mov eax,[var_C]; test; jz → 成功
0x1022e36a   失败: var_8 = 0; jmp 出口                         ← 不读 PLCF、不碰旗标
0x1022e373 var_8 = var_10                                      ← 类别
0x1022e398 call ATAINT(attr, [0x10B23D74] = 652066 PLCF, &var_10, &var_C)   ← 出参与第一次同一格
0x1022e3a0 mov eax,[var_10]; cmp eax,1; jz → var_4 = -1 else 0
0x1022e3bb mov dword_10E98500, var_4                           ← 旗标
0x1022e3e2 mov eax,[var_8]; retn                               ← 返回类别
```

边角：第二次 `ATAINT` 的 `var_C` **不检**，且 `var_10` 与第一次共用——`PLCF` 读失败时比的是残留的 DCHC 值（DCHC == 1 也会置旗标）。字典 4270 条都带 `PLCF`
（§3.2 导出时 58 个 1、其余 0），字典外属性在第一次 `ATAINT` 就失败早退，所以这条实际上碰不到；记下是为了换版本时别被它咬。

### 9.6 `PLCDEL`（Core3D `0x108e5890`）到底清什么

```
PLCDEL():
    t = dword_10E7BCE4            # { count = 100, slots[100], cursor, n }（sub_108E58D0 0x108e58d0 建表）
    for i < t.count: slots[i] 非空 → slots[i][+24] = 0        # 逐槽置「无效」
    t[2] = t[3] = 0
```

槽的形状（读端 `sub_108E4DB0` `0x108e4db0`）：按 `(DB_Ref @+28, int @+40)` 查（`sub_108E5470`），命中且键相等才返回 `+48` 起 24 字节 = 3 个 double。
读端 / 写端全在 `0x108E4D20`–`0x108E5980` 一段无符号 C++ 里，没有埋点名。

调用方（`xrefs → 0x108e5890`，10 处）：`descases/VDESPT` ×2（入口无条件 + 规则 V 改完 `DELP` 后）、`descases/VDESAC`、`change/UPDATD`（`dword_10E98500` 旗标为真时）、
`build/DEDRRP`、`csg_gml4/CTCDEL`、`sub_1024AA2D`、`sub_108E5DE0`、`0x10001ddd`、`0x100f3e96`。

**「p-line 位置缓存」是推断**，四条腿：(1) 字段名 `PLCF` 逆 hash 出来就叫 PLCF，`PLCDEL` 与它共用前缀 PLC；(2) 表里存的是按 `(元素, 整数键)` 索引的 3 维向量，
形状是「某元素第 n 条 p-line 的位置」；(3) `PLCF == 1` 的 58 个属性（`POS ORI BFORI SPRE HEIG PARA JOIS JOIE CFRA PFRE CUTB CUTP …`）恰好都是改了就让型钢 p-line 位置失效的属性；
(4) 规则 V 在 `pllib/GJDORG` / `PICOPS` / `GJTOR0` 算完节点位置、写完 `DELP` 后紧跟一次 `PLCDEL`。四条都不是字面证据，所以 §8 那句「只作报表、不进判决」不变。

### 9.7 ref 属性 put 的另一半：back-ref 维护（`VDESPF` / `VDESFA`）—— 0007 记录的「订阅者尚未定位」结案

- `VDESPF(el, noun, attr, target)`（`PostSetRefAttribute`，**写之前**发）：先 `structures/BAKREF(el, noun, target, attr, 0, 0)` 记 back-ref；
  然后一条特例：`noun == SPCO`（829324）且 `attr == PRTREF`（89651464）→ 经 `GATRF1(target, GPART, X)` 从部件读 `CATR DETR DETA MATX CMPR BLTR TMPR` 七个引用，
  逐个 `BPUTF` 到 SPCO 自己身上（读失败的跳过）。**设 `SPCO.PRTREF` 一次 = 七个属性跟着写**，目录侧写回规则，与 §9.2 规则 V / W 同族。
- `VDESFA(el, noun, attr, refs[], n)`（`PostSetRefListAttribute`）：`attr ∈ {PSARFA, PSLRFA, ALERFA}` 直接返回（这三个引用数组**不建 back-ref**）；
  `attr == COCORE` → 对每个非空新引用 `GATWR1(ref, COCDES, COCONN)` 取连接类型写进 `CTYP[i]`（`BPUTIA`），与 §9.2 规则 W 互为反向；
  其它 → 读旧列表（≤100），新有旧无 → `BAKREF`，旧有新无 → `structures/BREAKF`。
- 所以 0007 记录问的「`PostSetRefListAttribute` 只广播，真正写 back-ref 的订阅者是谁」：**`DESDRA_SCPlugs::PostSetRefListAttribute` → `descases/VDESFA` → `structures/BAKREF` / `BREAKF`**
  （Core3D，不在 core.dll）。ADR-003 那条线的 core 对照物到此闭合；对 gen-model 的判词不变（我们自建反向索引挂在落库写引用属性处）。

### 9.8 对 gen-model 的意义（接 §6 差距表）

| # | core 的规则（本节） | 我方现状 | 判 |
|---|---|---|---|
| G13 | 同值 put 不发任何事件（§9.1 第 1 步） | `diff_element` 五项逐字相同 → `unchanged`，不进候选 | ✅ 同构；且我们比 core 多挡一层——记录被重写但内容没变也不算 |
| G14 | `PLOO.HEIG` 当 owner 的变化评估（规则 U） | loop 容器成员的任何变化归属主重算（`is_loop_container_noun` 上溯） | ✅ 同构、更粗 |
| G15 | 属性 put 钩子里再写别的属性：`SJOI.JFRE` → `DELP`、`COCO.CTYP` ↔ `COCORE`、`SPCO.PRTREF` → 七个引用（规则 V / W / §9.7），与 0906 `RECON0` 同族 | 文件差分看到的是写回后的结果，同一窗里几个属性一起变；各自按效果表归类 | ✅ 不需模拟；解释语料里「一次操作多属性同窗」的一部分 |
| G16 | 空间地图包围盒只在 DCHC ≠ 0 或结构变化时更新（`MLSADD` 与 DCHC 门同一个 `if`）；进不进、算在谁身上由 noun 字段 `IMAP` 定 | Skip 档不上卷、不重算，空间树 / AABB 不动；`IMAP` 已在快照里（`noun_caps.json.spatial_map`），今天没有消费者 | ✅ core 侧多了一票：DCHC=0 的属性 core 自己也不动包围盒。`spatial_map` 有了语义，可作空间树「该不该给这个 noun 建盒」的字典判据候选（不排期） |
| G17 | 会话内路对 CATA 元素同样进 `EVALAT`（`DESDRA_SCPlugs` 只按 PADD 分流）：`SPCO` / `SVER` / `DATA` 变了 → `REPALL` | CATA 变更走依赖闭包 / 引用表反查，只重算受影响的 DESI 根（G9） | ✅ 比 core 细；核对表 R1（GETWORK 路只放 DESI / ISOD）不受影响 |
| G18 | 拷贝 / 排序 / include 直接 `EVALCD(el, 4)` 或 `EVALNW`，不经规则表联动；`BRAN` 内排序另记前一成员 | 结构变化（`Add` / `owner` / `members`）一律整根重算 | ✅ 更粗不漏；`BRAN` 排序两侧管段由整根覆盖 |
| G19 | `wnoevt` 属性不发变更事件；克隆属性把事件广播到全部克隆元素；分布式属性在持有元素上发 | 无对应概念：文件差分按记录看，写到哪条记录就在哪条记录上看到 | ⚪ 不可比；`wnoevt` 名单未导，若其中有 DCHC ≠ 0 的属性，core 会话内不重画而我们会重算——多算方向，安全 |
| G20 | 自动着色 / `designEvents` / 标注刷新（`RMKELE`）在 DCHC 门外照做 | 显示属性不在我方范围 | ⚪ |

没有一条改变 §5 三张答案表的判词：**「哪些属性触发几何重算」仍由 DCHC 类别 + `EVALAT` 规则表决定，`VDESPT` 只在它前面加了一道同样的门、一条上卷规则（U）和几笔写回。**

## 附：复查命令

```powershell
# 类别执行：UPGRPH / 只改矩阵的两条 / 整件重建
ida-bridge exec idalib-49136 --sql "SELECT decompile(0x1022eaee)"          # change/UPGRPH
ida-bridge exec idalib-49136 --sql "SELECT decompile(0x10232512)"          # css/STRTRA  → GLSMAT
ida-bridge exec idalib-49136 --sql "SELECT decompile(0x1023378b)"          # css/SEGTRA  → GLSMAT
ida-bridge exec idalib-49136 --sql "SELECT decompile(0x1021bebe)"          # add/REPELE  → TOPDEL + ADDDES
# IDCHNG 读的两个字典字段
ida-bridge exec idalib-49136 -c "import ida_bytes; _result_=[ida_bytes.get_dword(a) for a in (0x10B23D70,0x10B23D74)]"   # [596407, 652066] = DCHC, PLCF
# 缓存路只看 CACHID（逐指令）
ida-bridge exec idalib-49136 --sql "SELECT address, disasm FROM instructions WHERE func_ea = 0x1052c850 AND address BETWEEN 0x1052c905 AND 0x1052c934 ORDER BY address"
# GETWORK 路的候选只有索引级
ida-bridge exec idalib-33060 --sql "SELECT decompile(0x59834b0), decompile(0x5983c30)"
# noun / 属性 / 字段常量逆 hash（Python）
python -c "v=985369;n=v-531441;d=[];exec('while n:\n d.append(n%27);n//=27');print(''.join(chr(64+x) if x else ' ' for x in d))"   # DRAW

# ★0916 §9：属性 put 的入口链与 QCHGLS 写入者
ida-bridge exec idalib-49136 --sql "SELECT hex(from_ea) AS from_ea, name_at(from_func) AS func FROM xrefs WHERE to_ea = 0x1022C679 ORDER BY from_ea"   # EVALAT 的 8 处调用
ida-bridge exec idalib-49136 --sql "SELECT hex(from_ea) AS from_ea, name_at(from_func) AS func FROM xrefs WHERE to_ea = 0x1022E020 ORDER BY from_ea"   # EVALCD 的 21 处调用 = QCHGLS 全部写入者
ida-bridge exec idalib-49136 --sql "SELECT hex(start_ea) AS ea, name FROM funcs WHERE name LIKE '%DESDRA_SCPlugs%' ORDER BY start_ea"                  # 订阅者全部方法
ida-bridge exec idalib-49136 --sql "SELECT decompile(0x10409A60)"          # DESDRA_SCPlugs::PostSetAttribute → type 7 ? dra_scase/VDRSPT : descases/VDESPT
ida-bridge exec idalib-49136 --sql "SELECT decompile(0x101F33A9)"          # descases/VDESPT（DCHC 门 + 规则 U/V/W/X + 自动着色 + designEvents）
ida-bridge exec idalib-49136 --sql "SELECT address, disasm FROM instructions WHERE func_ea = 0x101F33A9 AND address BETWEEN 0x101f3419 AND 0x101f3510 ORDER BY address"   # DCHC 门 + PLOO.HEIG 逐指令
ida-bridge exec idalib-49136 -c "import ida_bytes; _result_={hex(a): ida_bytes.get_dword(a) for a in range(0x10B200D8, 0x10B20110, 4)}"   # VDESPT 常量表
ida-bridge exec idalib-49136 --sql "SELECT address, disasm FROM instructions WHERE func_ea = 0x1022EAEE AND (disasm LIKE 'cmp%' OR disasm LIKE 'j%' OR disasm LIKE 'call%' OR disasm LIKE 'test%') ORDER BY address"   # UPGRPH 分派骨架（312 条）
ida-bridge exec idalib-49136 --sql "SELECT address, disasm FROM instructions WHERE func_ea = 0x1022E302 ORDER BY address"   # IDCHNG 全函数
ida-bridge exec idalib-49136 --sql "SELECT decompile(0x108e5890), decompile(0x108E4DB0), decompile(0x108E58D0)"   # PLCDEL 与缓存表读 / 建
ida-bridge exec idalib-49136 --sql "SELECT decompile(0x10220F82), decompile(0x102208C7)"                         # maplist/MLSADD 与消费者 VDEMPR
ida-bridge exec idalib-49136 -c "import ida_bytes; _result_=ida_bytes.get_dword(0x10B22B80)"                      # 847458 = IMAP（MLSADD 读的 noun 字段 = 快照 spatial_map）
ida-bridge exec idalib-33060 --sql "SELECT decompile(0x593f220), decompile(0x59453b0), decompile(0x5992C80)"   # internalPutAtt(ref) / postSetAttribute / 受影响集
ida-bridge exec idalib-33060 --sql "SELECT hex(address) AS ea, name FROM names WHERE name LIKE '??_7DB_ElementChangesPlugger@@%'"   # vtable 0x5d19234：+52 PostSetAttribute · +60 PostSetRefAttribute · +64 RefList · +68 PostSetName
```
