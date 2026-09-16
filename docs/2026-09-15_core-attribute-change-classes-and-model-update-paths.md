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
| `DB_Element::hasElementChangedBetween` | `0x593d6d0` | `NOUN_TUBI` 只问 `ATT_POS / ATT_ORI / ATT_ITLE / ATT_SPRE` 四个；其它 noun 走 `sub_5AAD8E0`（dab 记录级「任一属性变」） |
| `DB_RawChanges::createListBetweenSessions` | `0x59834b0` | `switchToOldSession` → `DB_IndexTableCompare` → `iterate`（`0x5983c30`：按 `modified / inserted / 其余=deleted` 分三个 set）→ `getModifiedTypesAcrossSessions`。**只有索引级，没有任何属性判据** |
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

`DB_RawChanges::createListBetweenSessions`（`0x59834b0`）只有 `DB_IndexTableCompare`：记录在会话区间里被重写就是「修改」，
没有 `checkEle`、没有 `hasAttributeChangedBetween`；`sub_1052F2E0` 过 gb 门后 `FNDTOP` 折到顶层件，`EVALAT(top, REDRAW)` 类别恒 4，
`UPGRPH` 走 `top == ref` 分支 → `REPELE` 整件重建。**改 `NAME`、改 `DESC`、甚至内容一字未变只是被重写，顶层件都整件重画。**
唯一属性门在 `hasElementChangedBetween`（TUBI 四属性），但那是 `DB_Compare::checkEle` 用的，`DB_RawChanges` 这条路根本不调它。

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

- `UPGRPH`（`0x1022EAEE`）、`TRASCN`、`STRTRA`、`SEGTRA`、`SADREP`、`REPELE`、`IDCHNG`、`EVALST` 是 Hex-Rays 伪码 + `get_dword` 实读常量：调用序、埋点名、常量已核，
  **分支结构未逐指令核**。它们是 Fortran 风格平铺调用（`DGETI`/`DFIND`/`GLSMAT` 走导入表），没有核对表 §1.4 那类 `getField` 出参陷阱，伪码采信；但 `TRASCN` 里 `DDEST(-1)` 的语义
  （推定为「`DFIND NEXT` 自动下钻、不限深度」，与循环里 `DRETUR` 回上层的形状一致）只对了名表（opcode 37 = `DDEST`），**未读体**。
- `hasTopLevelGraphicsChanged` 是逐指令读的（Hex-Rays 把 `attributesChangedBetween` 的出参丢了，伪码恒 `return 0`，不可用）。
- `DES_DrawListCompare` 十二个回调读的是伪码，但都是 ≤ 10 行的小函数（空函数 / 一次 `set::insert`），可信。
- 「GETWORK 路不分属性」这句依赖 `DB_RawChanges` 只走 `DB_IndexTableCompare`（`createListBetweenSessions` 伪码）与 `sub_1052F2E0` 传 REDRAW（0906 逐指令）；
  `DB_RawChanges::trim(vector<DB_DB*>&, …)`（`0x5984650`）已读：对四个集合各套一个 `DB_Raw::PredicateDBNumMatch`——**只按库号裁**，没有属性判据；
  `trim(set<DB_Element>&)` / `trim(vector<DB_Element>&)` 两个按元素集合裁的重载没读体。
- `PLCF` 的语义只知道字段名（逆 hash）与「== 1 置 INTUBE 旗标」；~~字典里哪些属性 `PLCF == 1` **未导出**（P0-5）~~ ★0915 已导出（§3.2，58 个），
  但「置了旗标之后 `PLCDEL` 到底清哪些缓存」未读体——所以它仍只作报表，不进判决。
- `CACHID` 的写方不在本机任何 IDB 里。`D:\AVEVA\Everything3D2.10` 全目录（868 个 dll/exe）扫 `CACHID` 字节串 **0 命中，连 2.10 的 core.dll 里都没有这个属性名**
  （同一扫法 `ATT_HPOS` 命中 4 处，方法有效）——`CACHID`（与 `LCHKDA`）是 **3.1 才有的属性**，图形缓存路是 3.1 的新东西；而本机 3.1 只有 core.dll / libgeom / libgm 三个副本，
  Design 应用侧的 DLL 不在。§4.4 「写方在 Design 应用侧」是排除法推的，不是读出来的。
  ★0916 S8 从文件侧坐实了写方的**行为**（`+1` 计数、按应用分、成员增删跳属主、整库扫写），写方是谁仍未读出；「12476 个只 `CACHID` 且子树无变的
  跳动是缓存重建扫写」是从形状推的（同一会话里同 noun 全体 `+1`、同窗别无改动），没有读到写它的代码，也排除不了「被引用的目录件变了」这一种来源
  （探针只看 DESI 窗口，不看 CATA）。
- e3d-model 读取集合 131 个是**正则扫源码**（2–6 位大写字面量 ∩ 运行库属性名 − noun 名）的估算：漏掉 >6 位的长名与经变量传入的读点，也可能把同形常量算进来；P2-1 要用护栏测试把它变成准确的产物。
- 本文没有跑任何一条联动 / 分类的实测；§6 的 ✅ 是「规则形状同构」，不是本轮复验。

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
```
