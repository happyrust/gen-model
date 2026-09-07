# 2026-09-06 Core3D 联动更新的本体：`change/EVALAT` —— 一个构件变了，哪些别的构件跟着重画

- 问题（用户原话）：「要找到联动的增量更新，core3d.dll 里是如何处理一个构件的变更可能会影响其他构件的情况」。
- 手段：ida-bridge 活桥，`Core3D.dll` = `idalib-49136`、`core.dll` = `idalib-33060`
  （`D:\ida_scratch\plant3\`，E3D 3.1）。本文地址全部本轮实读；noun / 属性 hash 用 rs-core `db1_hash` 的逆函数
  （`db_tool.rs::db1_dehash_const`）与 `resource/dict/noun_caps.json` 解出。
- 承接：`docs/2026-09-04_model-cascade-update-inventory.md`（十一类联动表；本文改写它 §2.3 的
  「没有任何依赖递归」那一句）、`docs/specs/core3d-partial-update-conformance.md`（R5 说 DCHC 的作用
  「全在上游、决定进不进 QCHGLS」——本文找到的就是那个上游）、`docs/plans/2026-09-06-core3d-partial-update-conformance-dev-plan.md` W4.1。

---

## 0. 一句话

Core3D 的「联动」不在 C++ 的 `PartialUpdateDesiMgr` 里，也不在 `DDES_GraphicsMngr` 的过滤器里，而在
Fortran 血统的 **`change/` 模块**：

```
变更源（四种）                          评估                          结果队列              消费者
descases/VDESPT  属性 put  ─┐
descases/VDESDL  删除      ─┤
descases/SCINC2  include   ─┼─▶ change/EVALAT(元素, noun, 属性) ─▶ change/EVALCD ─▶ QCHGLS ─▶ UPDATN / UPDATD（3D 画列表）
change/EVALNW    新建      ─┤      ↑ DCHC 类别 + noun 专项规则       (EVALST: 步长 3,      PartialUpdateDesiMgr::
DDES_GraphicsMngr::postGetWork ────┘ (GETWORK: 先 FNDTOP 再以 REDRAW=4 进来)  类别取 max)        ChangedModelToUpdate（DRAFT）
```

**`EVALAT` 就是「一个构件变了、哪些别的构件要跟着动」的规则表**：先查属性的 DCHC 类别（`change/IDCHNG`），
再按 noun 走专项规则——上卷到顶层可画件（`FNDTOP`）、取 owner / 祖父、~~**沿正向引用**（`GATREF` 读 `CLIST` /
`ELCXRF`）~~ **★0907 C6：`GATREF` 读的 `CLIST` / `ELCXRF` 都是伪属性，底下是 dab 引用表的反向查**（`CWBRAN` 的 `CLIST` =
路过这条分支的电缆、`EQUI`/`ELCONN` 的 `ELCXRF` = `BACKRE{STAREF, ENDREF}` = 接在它上面的电缆，§8.6）、~~沿反向引用（`DFIND FIRST/NEXT` =
「谁引用我」）~~ **★0907 沿成员表向下**（`DFIND FIRST/NEXT` 是 dabacon 成员导航 = `db_move_down`，§8.1；**`EVALAT` 自己没有任何一条
反向引用调用**，反向只藏在 E/F 那两个伪属性里）、结构连接（`ADSTCH`：`CREF` ↔ `JOIS`/`JOIE`、
`CFRA` ↔ `PFRE` **双向核对**）、隐含管段（INTUBE → 前一个元件）——每命中一个目标就
`EVALCD(ref, 类别)` 推进 QCHGLS。

GETWORK 也走这条路：`sub_1052F2E0` 对每个变更元素先 `FNDTOP`（按 **`toplevel`** 位找顶层可画件），
再对「owner 不在集合里」的顶层件调 `EVALAT(top, noun, 331445106=REDRAW)`。所以 09-04 清单 §2.3
「GETWORK 全程没有任何依赖递归」**要改成**：过滤器自己没有，但它把每个顶层件交给 `EVALAT`，
`EVALAT` 里有引用规则（~~EQUI 的 `CLIST`、CWBRAN 的 `ELCXRF`~~ **★0907 C6 改正：CWBRAN 的 `CLIST`、EQUI/ELCONN 的 `ELCXRF`**——0906
把两个属性写反了，见 §8.6、~~CTRAY / 网格连接点的反向引用、
设备子件与 PANE 的「谁引用我」~~ ★0907 后三条实为**成员表向下**，见 §8.1；真正跨子树的只有 `CLIST` / `ELCXRF`（二者经引用表反向到电缆）与 §3 的结构连接）。

## 1. 地址表（E3D 3.1）

| 符号 / 埋点名 | 模块 | 地址 | 大小 | 角色 |
|---|---|---|---|---|
| `change/EVALAT` | Core3D | `0x1022C679` | `0x10fb` | **联动规则表**，本文主角 |
| `change/EVALCD` | Core3D | `0x1022E020` | `0x87` | 把 (ref, 类别) 交给 EVALST |
| `change/EVALST` | Core3D | `0x1022E0A7` | — | 写 QCHGLS：`HQLNIR` 取长度、`i=1; i+=3` 扫、`EQREF` 命中则 `HPUTI1` 取 **max** 类别，否则 `HPUTIA` 追加 (ref₀, ref₁, 类别) |
| QCHGLS 句柄 | Core3D | `dword_10E98540` | — | EVALST 写、`UPDATN`（`0x1022E3E7`）/ `UPDATD`（`0x1022E5AC`）读；核对表 R5 说的「步长 3」就是它 |
| `change/IDCHNG` | Core3D | `0x1022E302` | `0xe5` | `ATAINT(属性)` 读属性的 DCHC 整数；`==1`（INTUBE）时置全局 `dword_10E98500 = -1` |
| `change/EVALNW` | Core3D | `0x1022C43D` | — | 新建元素 → EVALAT |
| `descases/VDESPT` / `VDESDL` / `SCINC2` | Core3D | `0x101F33A9` / `0x101F1B24` / `0x101F2638` | — | 会话内属性 put / 删除 / include（各调 EVALAT 两次） |
| `change/ADSTCH` | Core3D | `0x1022D774` | `0x538` | 结构连接联动（§3） |
| `change/EVALRF` | Core3D | `0x1022DCAC` | — | 「沿属性 A 到 X，X 的属性 B 是否指回我」的双向核对，命中就 EVALCD |
| `connect/RECON0` | Core3D | `0x1025F521` | — | ADSTCH 命中后的重连（★0907 实读 §8.2：不查表，沿 `CREF` / `JOIS` / `JOIE` 导航后**写回**两端的连接属性） |
| `connect/STRCNT` / `JTCONN` / `FXCONN` | Core3D | `0x102616EA` / `0x10261B5B` / `0x1026358C` | — | RECON0 的工人：断开 / 节点重连 / FIXING 重连（§8.2） |
| `structures/SETREF` / `MPUTR` | Core3D | `0x102D43CC` / `0x102D5D38` | — | 写 `JOIS` / `JOIE` 引用、写实数属性（§8.2） |
| `DFIND` 本体 | **core.dll** | `0x5aaff40` → `sub_5ACFF70(43)` → **`sub_5AB6CB0`** | — | ★0907 §8.1：**成员导航**（与 `db_move_down` 同一个体），列表由 `sub_5AEA430` → `sub_5AEA500(记录, kind=2)` 从记录的成员块建 |
| `DB_Element::hashValue` | core.dll | `0x593dd00` | — | ★0907 §8.3：= `DB_Noun::hashValue(hardType(noun))`，即 **hard type 的 noun 词 hash** |
| `ENHAS2` | core.dll | — | — | 词 hash 本体（§8.4）：`531441 + Σ (c_i) · 27^i`，A–Z → 1..26、空格 → 0、其它字符整个 hash 判无效 |
| `DES_DrawListManager::updateGraphics` / `UpdateChangeList` | Core3D | `0x1052D330` / `0x1052B3C0` | — | ★0907 §8.5：`UPDATN` / `UPDATD` 到 3D 画列表的那一跳；`UpdateChangeList` 是 QCHGLS 的消费者 |
| `desdblib/FNDTOP` | Core3D | `0x10380E38` | `0x21f` | 从 CE 沿 owner 上行，找第一个 `LISTOP` 为真的元素；顶层件的 owner 若是 `TMPL` 则改取 owner |
| `desdblib/FNDEQP` | Core3D | `0x1038174C` | `0x244` | 点类 noun（`LPOINT`）→ 所属几何件 |
| `nounlib/LISTOP` | **core.dll** | `0x561C93C` | — | 顶层可画件判据：读 noun 逻辑位 **`0xA187C`（`toplevel`）**；为假时退到 `0xA103E`（`primitive_a`）配特例（§4） |
| `INCOMP` / `IPCOMP` / `IHCOMP` / `IECOMP` / `LPOINT` | core.dll | `0x561C1B0` / `0x53A9A10` / `0x53A9AAC` / `0x561BC58` / `0x561CD78` | — | noun 谓词（管道 / 管道 / HVAC / 设备子件 / 点类），本轮**未读体** |
| `DFIND` / `GATREF` / `XTREE` / `CGETEL` / `CLIMBM` / `CLIMBA` / `NATTA` | core.dll（Core3D `.idata` 导入） | — | — | dabacon 导航原语：~~反向引用 FIRST/NEXT~~ **成员 FIRST/NEXT/PREV/LAST/SAME（★0907）**、取属性引用数组、子树遍历、按 noun 取祖先、按 noun 表爬升、前后元件。`change/` 模块（`0x1022C000`–`0x1022F000`）调 `DFIND` 19 次、`DRETUR` 10 次，**一次都没调** `db_find_reference`（opcode 46）或 `DB_RefTableIterator` |
| GETWORK 过滤器 | Core3D | `0x1052F2E0` | `0x377` | 09-04 清单 §2.3；本文补上它第二段的输入集合与事件消费者（§5） |

## 2. `EVALAT(el, noun, attr)` 的规则表

伪码按 Hex-Rays 输出核过调用与常量（这段是 Fortran 风格的平铺调用，不是 §1.4 那类丢分支的 `getField` 形状；
但仍属「伪码 + 常量解码」而非逐指令，见 §7）。`类别` = DCHC 类：`1` INTUBE、`2`、`3` 位姿、`4` 通用几何 / REDRAW。

### 2.1 类别从哪来

| `attr` 入参 | 类别 | 说明 |
|---|---|---|
| `331445106`（`0x13C17372`） | **4** | GETWORK / Undo / Redo 的强制 REDRAW（`sub_1052F2E0` 传） |
| `73767168` | **1** | 强制 INTUBE |
| 其它 = 属性 hash | `IDCHNG(attr)` | 字典里该属性的 DCHC 整数（ADR-002 说的「码在字典里」，读点就是 `ATAINT`） |

类别为 0 → 什么都不做（**对应核对表 R13「双假丢弃」的上游：DCHC=0 的属性根本不进队列**）。

### 2.2 noun 专项规则（按伪码顺序，全部命中都执行、不互斥）

| # | 变更元素的 noun | 规则 | 走的边 | 被标记的目标 | 类别 |
|---|---|---|---|---|---|
| A | `FLRLAY` `WLOPEN` `HRGATE` `RAIL` `KICKPL` `HRPOST` `BPOPEN` `AIDLIN` `AIDARC` `AIDCIR` `AIDPOI` `AIDTEX` + 1 个死 noun（`271245643` = `ENHAS2("YASJWR")`，3.1 字典里没有，§8.4） | `FNDTOP` | owner 链 | 顶层可画件 | 4 |
| B | `GRIDPL` `GRIDCY` `GRIDEL` `GRIDFA` | `CGETEL(REFGRD)` → `XTREE` 遍历该网格子树 | 正向引用 `REFGRD` + 子树 | 子树里 `GRIDPL` / `GRIDCY` 的 `REF`（先过 `sub_1050D810` 谓词，**未读**） | 4 |
| C | `CGRDCP` `CGRDLP`（网格连接点） | `DFIND FIRST/NEXT` | ~~反向引用（谁引用我）~~ **★0907 成员表**（`DFIND` = dabacon 成员导航，§8.1） | **成员**里 noun ∈ {`FPFITT` `ELFITT` `HVACFI` `INFITT`} 的 | 继承 |
| D | `CTRAY` | `DFIND FIRST/NEXT` | ~~反向引用~~ **★0907 成员表** | **全部成员** | 4 |
| E | `CWBRAN` + 1 个死 noun（`269526523` = `ENHAS2("VWIDTR")`，§8.4） | `GATREF(CLIST)`（**★0907 C6 改正**：0906 写成了 `ELCXRF`；实读常量 `unk_10B23C98` = 11541126 = `CLIST`） | ~~正向引用数组~~ **伪属性**（Core3D `app::cab::CAB_DB_BranchCableList_pseudo`）：分支下 `HATTA`/`TATTA`/`RNODE`/`RATTA` 成员的 `CLIST` ∪ `LALCAB`，底下 `LALCAB::takeCables` 逐库 `DB_RefTableIterator(STAREF \| ENDREF)` **反向**找引用这些节点的 `EXTGEO` 段、owner 两级到 `CABLE`（§8.6） | 路过这条分支的电缆（≤2000） | 4 |
| F | `EQUI` `ELCONN` | `GATREF(ELCXRF)`（**★0907 C6 改正**：0906 写成了 `CLIST`；实读常量 `unk_10B23CA0` = 96665729 = `ELCXRF`） | ~~正向引用数组~~ **伪属性**（core.dll `DB_PseudoELCXRF_DESI` = `getAtt(BACKRE, {ENDREF, STAREF})` → `DB_PseudoBACKRE` 逐库 `DB_RefTableIterator`）：**引用表反向查**「`STAREF`/`ENDREF` 指向我的元素」，跨库也算（§8.6） | 接在它上面的电缆（`CABLE.STAREF`/`ENDREF` 的持有者，≤2000） | 4 |
| G | `CLNPNG` | `DFIND` 两层（伪码里两个 `DFIND FIRST … DRETUR … DFIND NEXT` 循环嵌套） | ~~反向引用 ×2~~ **★0907 成员表两层** | 成员全部；成员若是 `CLNCGR`，再标它的 `CLNTIL` 成员 | 继承 |
| H | 类别 == 1（INTUBE） | `CRETUR` + `NATTA(PREV)` | 分支序 | **前一个元件**的 `REF`（它的隐含管段变了） | 4 |
| I | 类别 == 2 | — | — | 自身 | 4 |
| J | `INCOMP(noun)`（noun 位 `HOLC`：**负实体 / 孔洞构件**，`DTHOLE` 除外） | — | — | 自身 | 4 |
| K | 在 `STRU` 之下（`CGETEL(STRU)` 成功） | `ADSTCH(el)` | 结构连接（§3） | 连接对端 / 顶层件 | 见 §3 |
| L | `LISTOP(noun)`（noun 位 `TOPF`，§4） | — | — | 自身 | 继承 |
| M | `CLIMBM({PANE, GENSEC, SCTN, BPANEL})` **成功**（自身或祖先是这四种之一） | owner 链 | 上卷 | **那个 PANE / GENSEC / SCTN / BPANEL**（结构侧的「元件归分支」），直接跳 §2.3 | 4 |
| N | `FIXING` | `FNDTOP`，顶层件也是 `FIXING` 才算 | owner 链 | 顶层件 | 4 |
| O | `FPFITT` `ELFITT` `HVACFI` `INFITT` | owner 是 `CLNTIL` 才算 | owner | owner | 4 |
| P | `INCOMP`（`HOLC` 孔洞）∨ `IPCOMP`（`PIPC` 管道元件）∨ `IHCOMP`（`HNGC` 支吊架元件），或 `PLTGRD` `INTFRM` | — | owner | **owner**（元件 → 所属分支 / 容器） | 4 |
| Q | `SUBCOM` | — | owner.owner | 祖父 | 4 |
| R | 类别 ∈ {3,4} 且 `PLOPEN` `DPCA` `DPCY` `DPSP` `DPSE` | `FNDTOP` | owner 链 | 顶层件 | 4 |
| S | 类别 ∈ {3,4} 且 `LPOINT(noun)`（noun 位 `POPF` = 字典 `point`） | `FNDEQP` | owner 链 | 点所属的几何件 | 4 |
| T | `POHE` | — | — | 自身 | 4 |

规则 M 到 T 是一条 `if / else if` 梯子（M 成功就不看 N–T）；A–L 各自独立、命中就执行。

### 2.2a 五个 noun 谓词读的是哪个字典位（core.dll `nounlib/*`，全部实读）

| 谓词 | 读法 | noun 字段 hash | 字段名（逆 hash） | 语义 | 我方对应 |
|---|---|---|---|---|---|
| `INCOMP` | `ATNINT` ≠ 0，且 noun ≠ `DTHOLE` | `0x92663` / 599651 | **`HOLC`** | 负实体 / 孔洞构件 | 核对表 R0 表里叫 `IsNegative` 的就是这个位（`0x92663`）——名字对上了：negative = hole component |
| `IECOMP` | `ATNINT` ≠ 0 | `0x94037` / 606263 | **`EQUC`** | 设备子件 | 09-04 清单「同族字段语义未追」之一，已解 |
| `IPCOMP` | `ATNINT` ≠ 0 | `0x9312d` / 602413 | **`PIPC`** | 管道元件 | 同上，已解 |
| `IHCOMP` | `ATNINT` ≠ 0 | `0x9180b` / 595979 | **`HNGC`** | 支吊架元件 | 同上，已解 |
| `LPOINT` | `ATNLOG` | `0xa1878` / 661624 | **`POPF`** | 点类 noun | = 字典 `point`（`noun_caps.json`，44 个；T2b 的 `is_loop_container_noun` 读的就是它） |
| `LISTOP` | `ATNLOG` + 特例 | `0xa187c` / 661628 | **`TOPF`** | 顶层可画件 | 核对表 R0-3 的 `toplevel`（211 个） |
| （参照）`IsPrimitive` | — | `0xa103e` / `0xbbd5adc` | `PRMF` / `OWNPRM` | primitive_a / primitive_b | 快照两位 |
| （参照）`IsSignificant` | — | `0x5657a0a` | `DRSIGF` | 绘制显著位 | 快照 `significant` |
| （参照）`graphicsBehaviour` | — | `0x4dce6f` | `GRAPH` | 0/1/2/3 | `graphics_behaviour` |

（另两个同族字段 `847458` = `IMAP`、`599813` = `HULC`（船体构件），本轮无消费者出现。）

### 2.2b 入口传的「属性」是什么

| 入口 | 传给 EVALAT 的 `attr` | 含义 |
|---|---|---|
| `descases/VDESPT`（属性 put） | 真正被 put 的属性 hash（两次：元素自己、以及相关的另一元素 `&v10`） | 走字典 DCHC |
| `descases/VDESDL`（删除）/ `descases/SCINC2`（include） | 先 `73767168`（INTUBE），再 `331445106`（REDRAW） | 删 / 改挂：先算前一元件的管段，再整件重画 |
| `change/EVALNW`（新建） | `331445106`（REDRAW）；仅当 `sub_1021DB75` 判真才进 EVALAT，否则直接建图形 | 新建走「有图形就直接建、否则按变更评估」 |
| `sub_1052F2E0`（GETWORK / Undo / Redo） | `331445106`（REDRAW），元素先 `FNDTOP` | §5 |

规则 B 的过滤谓词 `sub_1050D810` = `DES_DrawList::exists(currentDrawList, el)`——只标**当前画列表里有的**网格件。

### 2.3 收尾：目标定了之后还要再看一眼

设上面得到的目标为 `T`（缺省 = 自身）：

1. `T.type == TMPL` → 取 `T.owner`，再 `FNDTOP` → 标记那个顶层件。
2. 否则若 **不是** `IECOMP(T.type)`（`EQUC` 设备子件）且 `T.type != PANE`（或全局旗标 `dword_10EB4004 & 0x10`）→ 直接标记 `T`。
3. 否则（**T 是设备子件或 PANE**）→ `DFIND FIRST/NEXT` 遍历 ~~所有引用 T 的元素~~ **★0907 T 的全部成员**（`DFIND`
   是 dabacon 成员导航，不是反向引用，§8.1），其中 `INCOMP`（`HOLC` **孔洞 / 负实体构件**）为真的逐个标记——即
   「**被打孔的板 / 设备子件动了，它名下的孔洞构件跟着重算**」；随后 `T` 自身也标记。（第一版本文把这条读成
   「接在 NOZZ 上的管道元件」，**错了**：`INCOMP` 读的是 `HOLC` 不是 `PIPC`，见 §2.2a；第二版读成「引用 T 的孔洞构件」，
   **也错了**：走的是成员表，孔洞构件是 T 的子件。管道连接在 core 里不走这条——`HPOS`/`TPOS` 是存储属性，09-04 清单 §2.9。）

## 3. 结构连接：`change/ADSTCH`（`0x1022D774`）

`EVALAT` 规则 K。对元素 `el` 与它子树里的每个后代（`XTREE`），做三组**双向核对**（`EVALRF(ref, A, n, B)` =
读 `el.A[1..n]` 得 X，逐个到 X 读 `X.B`，**等于 `el` 才算连接成立**，成立就标记 `FNDTOP(X)`——若顶层件是
`STRU` / `SUBS` / `FRMW` / `SBFR` 则改标 X 自己）：

| 组 | 前置 `CLIMBA` | 正向属性 A（读几个） | 回指属性 B | 命中后 |
|---|---|---|---|---|
| 1 | `CFRA` 祖先 | `CFRA`（≤10） | `PFRE` | 标记 |
| 2 | `CREF` 祖先 | `CREF`（1） | `JOIS`，不中再试 `JOIE` | 标记 + `RECON0(RECO, JOIS/JOIE, 0, -1)` 重连 |
| 3 | `JOIE` 祖先 | `JOIS`（1）与 `JOIE`（1） | `CREF` | 标记 + `RECON0(RECO, BOTH, …)` |

即：**型钢 / 节点（SCTN ↔ 节点的 `JOIS`/`JOIE` ↔ 对端 `CREF`）之间的连接是核对过「你指我、我也指你」才联动的**，
单向的悬空引用不触发。这是核对表与 09-04 清单都**没有**登记的一类联动边。

## 4. 「顶层可画件」到底是哪个位：`LISTOP` 读 `toplevel`

`nounlib/LISTOP`（core.dll `0x561C93C`）逐行：

```
top = ATNLOG(noun, 0xA187C /* toplevel */)
if top:
    if noun == WORL: top = false
    elif owner.type == TMPL: top = false            # 模板下的顶层件不算
else if ATNLOG(noun, 0xA103E /* primitive_a */):
    if noun in {RPLA, PVOL, DRAW}: top = owner.type in {SITE, ZONE}
    elif noun == FIXING: 沿 owner 上行到第一个 toplevel 或 FIXING；到达 STRU 才算 top
    # 其它 primitive：不算 top
```

`FNDTOP` 就是「含自身沿 owner 上行到第一个 `LISTOP` 为真」，再加一条「顶层件的 owner 是 TMPL 就取 owner」。

**对核对表的意义**：核对表 W4.1 问「GETWORK 之后重画的单位是 TOPF 项还是 significant 段」——**答案是 TOPF
（`toplevel`）**：GETWORK 的每个变更元素先被 `FNDTOP` 折叠到 `toplevel` 件，再进 `EVALAT` → QCHGLS。
`significant` 只在 DRAFT 侧 `PartialUpdateDesiMgr::GranularityExpansion` 消费 QCHGLS 时才出场。
核对表 R0-3「我们用 `significant` 选根是对的」这句仍成立（它说的是几何段的索引），但**3D 视图的重画单位是
`toplevel`**，两者之间的 211 / 127 / 交集 93 差集（`docs/evidence/2026-09-04-noun-toplevel-reconciliation.md`）
现在有了一个具体的消费者。

## 5. GETWORK 路径补全（09-04 清单 §2.3 第二段的两个「未坐实」）

`sub_1052F2E0(this)`：

1. **第一段的插入点坐实了**（`0x1052f443`–`0x1052f481` 逐指令）：对每个过了 gb 门的变更元素
   `db_go_to_element(el)` → **`FNDTOP(flag=0, &top)`** → `DB_Element top` 非空 → `sub_1046B7E0`（`ecx = &v46`，
   `std::set<DB_Element>::insert`）。**插进结果集的是 `FNDTOP(el)`，不是 `el` 自己**——09-04 清单写的
   「剩下的插进一个有序结果集」要改成「剩下的先折叠到顶层件再插」。
2. **第二段的消费者坐实了**：对结果集逐个 `owner` 做 lower_bound，owner 不在集合里的才调
   `sub_1022C679(asPointer(top), &hashValue(top), &331445106)` —— 这不是发事件，**就是直接调 `change/EVALAT`**
   （`0x1022c732: cmp eax, 13C17372h` 是 EVALAT 自己在判类别）。于是 GETWORK 的每个存活顶层件都走一遍 §2 的
   规则表，以 REDRAW=4 为类别。

三个入口共用（`postGetWork` / `postUndo` / `preRedo`，`op` 4/5/6，09-04 清单 §2.1）——所以 **UNDO / REDO 在 3D
侧的联动口径与 GETWORK 完全相同**（W4.1 第 4 条问的那半句）。

## 6. 对 gen-model 的意义（对着 09-04 清单 §1 的十一类）

| 清单类 | core 这边的对应规则（本文） | gen-model 现状 | 判词 |
|---|---|---|---|
| #5 连接 / 邻居 | 规则 H（INTUBE → 前一元件）。**管道连接（HREF/TREF）在 EVALAT 里没有任何一条**——与 09-04 §2.9「`HPOS`/`TPOS` 是存储属性、接管时回写 BRAN」互证 | INTUBE 在 BRAN 内由整根重生成覆盖 | 09-04 判「构造性不可达」成立，且多了一条正面证据 |
| 新 #15 孔洞 / 负实体 | §2.3 第 3 条：PANE / 设备子件（`EQUC`）变了 → ~~`DFIND` 找引用它的孔洞构件~~ **★0907 遍历它的成员**，`HOLC` 的逐个重算；规则 J / P：孔洞构件自己变了 → 自身 + owner | 负实体在我们这边是 owner 子树内的成员（整根重生成覆盖 owner 内的孔） | ~~❓ direct 路径缺反向级联~~ **★0907 ✅ 构造性覆盖**：core 自己也只看成员表、不查引用者；`DFIND` 不是反向引用（§8.1）。0907 真库普查另证「跨根孔洞引用 = 0 / 零窗口」（`2026-09-07-cross-root-reference-census.md`）——两边一致 |
| #4 CATA → DESI | **无**：`EVALAT` 里没有一条走 `SPRE`/`CATR`；GETWORK 库过滤只放 DESI/ISOD | `ref_rev` / 指纹失配 | 09-04 结论不变：我们比 core 强，无对照物 |
| #9 UDA / #10 XRPNTR | **不在 EVALAT 里**（那两条在 `DB_UserChanges::attributeModified`，ADR-061） | 不做 | 不变 |
| **新 #12 结构连接** | §3 `ADSTCH`：`CREF` ↔ `JOIS`/`JOIE`、`CFRA` ↔ `PFRE` 双向核对后联动对端。注意它是**从变更元素出发的正向读**（读 `el.CREF` 到 X，再读 `X.JOIS` 核对），**不需要反向表** | **未登记**。旧 PE 路径：`manual_update.rs::reference_edge_eligible` 按 schema `att_type == ELEMENT` 收边，`CREF`(35 noun) / `JOIS` / `JOIE`(4) / `CFRA`(3) / `PFRE`(1) 都是 ELEMENT 型 → 进 `ref_rev`，对端的交付单元会级联（不做双向核对 = 超集，多做不漏）。**direct 文件窗口路径：没有任何反向级联**（09-04 清单 #4 同一个洞：`direct_store::referrers_of` 零调用方） | ~~❓ direct 路径上是真缺口候选~~ **★0907 已坐实并接线**：真库普查 21,161 条回指成立的跨根边（`2026-09-07-cross-root-reference-census.md`）；`src/data_interface/structural_connection.rs` 正向读 + 回指核对 + `STRU` 门，开关 `model_structural_connection_cascade` 默认关（提交 `1913d224`）。§8.2 补：core 命中后还 `RECON0` **写回**两端连接属性，所以真实会话里两端常常都进账本，C2 补的是写回没改到型钢字节的那些窗口（普查 12 个） |
| 新 #13 设备连接表 | 规则 F：`EQUI` / `ELCONN` 变了 → ~~`CLIST` 数组~~ **`ELCXRF`**（★0907 C6：0906 写反了属性）= 引用表**反向**找 `STAREF`/`ENDREF` 指向它的元素（= 接在它上面的 `CABLE`）→ 全部 REDRAW | ~~`CLIST` 不在存储属性里……多半是伪属性~~ **★0907 C6 定性完（§8.6）**：`CLIST` / `ELCXRF` 都是伪属性、底下都是 `DB_RefTableIterator`；这是一条**反向边**，不是正向数组。direct 文件窗口路径：无；旧 PE 路径：`CABLE.STAREF`/`ENDREF` 是 schema ELEMENT 型 → 进 `ref_rev` → 电缆单元会级联（超集） | ~~❓ 待核~~ **★0907 C6：语料里有实例**——ELCONN 1607 / CABLE 116 / `CABLE.STAREF`·`ENDREF` 边 36 条同库跨根 + 196 条跨库（§8.6）→ 按计划 §3 C6 的判词「有 → 另开计划」：登记为 **C8 提案**（电缆端点 / 路由反向级联），今天电缆几何没按路由落地（`CABLE` 现走 cata_model，`EXTGEO` 段未接），电缆几何落地即成正确性项 |
| 新 #14 桥架 / 网格 | 规则 B–E、G（~~`ELCXRF` 正向~~ **E = `CWBRAN` 的 `CLIST` 伪属性 = 路过的电缆，引用表反向**；B = `REFGRD` 正向；C/D/G = `CTRAY` / 网格连接点成员向下） | 桥架族 `gb==3`（09-04 §2.5），几何覆盖度问题；E 那条与 #13 同源（同一张引用表、同两个属性 `STAREF`/`ENDREF`，只是从路由节点那头查），归 C8 提案一起判 | 登记不排期（E 并入 C8 提案） |
| 顶层单位 | §4 `toplevel` | 生成根按 `significant` | 不改判据（核对表 R0-3），但 3D 重画单位 = TOPF 这条要写进核对表 W4.1 的结论 |

**最值钱的一条是 #12**：它是 core 明确做了、而我们的十一类表上根本没有的一条边，且属于 DESI 三维模型增量的范围。

## 7. 不许当已证

- §2 / §3 的规则来自 Hex-Rays 伪码 + 常量解码。这批函数是 Fortran 风格的平铺调用（`DGETF` / `DFIND` / `GATREF`
  全走导入表，无 `getField` 出参陷阱），伪码里**没有**出现 §1.4 那类孤零零的 `getField(v); return 0` 形状，
  所以采信；但**没有逐指令核**。`sub_1052F2E0` 的插入点（§5.1）是逐指令核过的。
- ~~`DB_Element::hashValue(el)` 被当作「noun hash」用……没有读 `hashValue` 的实现~~ **★0907 已读（§8.3）**：就是 noun 词 hash，
  且是 **hard type** 的（UDET 折回底层 noun）。
- ~~`DFIND(FIRST/NEXT, 1)` 读作「反向引用」……它查哪张表未读~~ **★0907 已读（§8.1），而且原读法是错的**：`DFIND` 是
  dabacon **成员导航**（与 `db_move_down` 同一个体），列表从元素记录的 kind-2 = 成员块建；不查任何引用表。
  §2.2 C/D/G 与 §2.3 第 3 条已按此改正。
- ~~五个谓词未读体~~ 已读（§2.2a）；`sub_1050D810` 已读（= `DES_DrawList::exists`）；`CLIMBM` 表已解
  （{PANE, GENSEC, SCTN, BPANEL}，`0x10D906D0` = 计数 4）。
- ~~两个 noun 常量 `271245643`、`269526523` ……未解~~ **★0907 已解到能解的尽头（§8.4）**：两个都是合法 `ENHAS2` 词 hash
  （`YASJWR` / `VWIDTR`，字母表只有 A–Z 与空格，解码唯一），3.1 字典里没有这两个 noun → 死分支；不是 UDA / UDET
  （那两族的 hash 在 `27^6 + 27^4 = 387951930` 之上）。
- 类别语义 `1` = INTUBE、`4` = REDRAW 沿用 ADR-002 的静态结论；`2` 与 `3`（位姿）按
  `src/data_interface/dchc_change_classes.json` 的口径，本轮未从二进制重证。
- ~~dab 引用表只覆盖 kind-16 而 `DFIND` 能找到 `HREF`/`TREF` 引用者的矛盾~~ **★0907 消解**：`DFIND` 根本不查引用表，
  所以两条结论都对——引用表只有 kind-16 属性，~~`EVALAT` 一次也没查过它~~ **`EVALAT` 自己一次也没查过它；但 E/F 两条经伪属性
  `CLIST` / `ELCXRF` 间接查了（§8.6），查的属性是 `STAREF`/`ENDREF`，正是 kind-16 那族**。ADSTCH 不受影响（它是正向读）。
- ~~`CLIST` 多半是伪属性、由谁算未读~~ **★0907 C6 已读（§8.6）**：`CLIST` / `ELCXRF` 都是伪属性；`DB_PseudoELCXRF_DESI`、`DB_PseudoBACKRE`、
  `CAB_DB_PseudoLALCAB::takeCables`、`CAB_DB_PseudoCLISTonCNODE` 体很短，伪码可信；`CAB_DB_BranchCableList_pseudo::getAtt` 与
  `sub_109C9090`（qualifier / `CBSTA` 状态过滤）只看了调用序与属性导入，**没有逐指令核**。0906 把 E/F 两条的属性写反了，
  是伪码里 `unk_10B23C98` / `unk_10B23CA0` 两个常量对号时错位——0907 用 `ida_bytes.get_dword` 直读后按 `db1_dehash` 解回 `CLIST` / `ELCXRF`
  改正。语料那一半：`CABLE.STAREF`/`ENDREF` 的 196 条跨库边对端 noun **未逐条解**（普查 JSON 只记 `@desi<dbnum>`），只知道对端库就是
  语料里含 ELCONN 的那六个电气库；「只改 EQUI/ELCONN 一端、电缆不动」的窗口**未量**（普查的窗口统计只跟了结构边与孔洞边）。
- **没有跑任何一条联动的实测**。§6 的 ❓ 三行是取证结论，不是缺口结论。（0907 后 #12 已由真库窗口实测：
  `structural_connection::tests::live`；#15 判构造性覆盖，不需要实测。）
- §8 的读法：`RECON0` 一族与 `sub_5AB6CB0` / `sub_5AEA500` 是 Hex-Rays 伪码 + 常量解码，**未逐指令核**；`ENHAS2`、
  `DB_Element::hashValue`、`DRETUR` 体很短，伪码即可信。`sub_5AEA500` 里 kind-1 / kind-2 两个槽位哪个是成员，
  由页块头的 kind 标签校验（`((hdr >> 16) & 0xF) != kind → 错误 551`）与 e3d-io `record/block.rs`（`2 => Members`，
  438 库 3,148,449 块实测）双证。

## 8. 0907 补洞（C7）：`DFIND` / `RECON0` / `hashValue` / 两个死 noun / `UPDATN` 之后那一跳

全部本轮实读，桥同 §1（`Core3D.dll` = `idalib-49136`、`core.dll` = `idalib-33060`）。

### 8.1 `DFIND` 查的不是表，是**成员列表**——`EVALAT` 里没有反向引用（直接调用层面；E/F 经伪属性那一层见 §8.6）

```
DFIND(dir, n)          core.dll 0x5aaff40   = sub_5ACFF70(43, dir, n)        ← 43 只是 trace 名表的下标（off_6003900[43] = "DFIND"）
db_move_down(dir, n)                        = sub_5ACFF70(44, dir, n)        ← 同一个派发器、同一个体
sub_5ACFF70(op, a, b)  = trace(op) ; sub_5AB6CB0(a, b, 1)                    ← 体不看 op
sub_5AB6CB0(dir, n, 1) core.dll 0x5ab6cb0:
    frame = CE 栈顶（dword_6A54024 + 60 * level）
    list  = frame[6]  ?: sub_5AEA430(0)                                      ← 懒建：sub_5AEA500(记录, kind = 2, …)
    dir ∈ { FIRST 11547609, NEXT 942746, PREV 968614, LAST 938991, SAME 639379 }（ENHAS2 逆解）
    pos   = FIRST: n | NEXT: cur + n | PREV: cur − n | LAST: count − n + 1 | SAME: cur
    refno = list[pos]（8 字节一条）；sub_5AEB6B0(refno, 2) 把 CE 移到它
DRETUR()               = sub_5AD91F0(183) → sub_5AEB0B0()                    ← 要求 level ≥ 1，sub_5AE9460(0, 2, 0) 退一层 = 回到 owner
                                                                              （EVALAT 循环里每轮 DRETUR 再 DFIND NEXT，正是「看完一个成员回 owner 再取下一个」）
sub_5AEA500(rec, kind, …) core.dll 0x5aea500:
    kind 1 → 槽 rec[6..7]，计数 (rec[10] >> 14) & 0x3FFF      ← 显式属性流
    kind 2 → 槽 rec[8..9]，计数  rec[10]        & 0x3FFF      ← **成员表**（0x3FFF = 溢出，逐页数）
    每页块头 ((hdr >> 16) & 0xF) 必须 == kind，否则错误 551    ← 与 e3d-io record/block.rs 的 BlockKind 编号一致：2 = Members、1 = Explicit
```

所以 `EVALAT` 里那 19 处 `DFIND FIRST … DRETUR … DFIND NEXT` 都是「逐个成员、看完回 owner」。`change/` 模块
（`0x1022C000`–`0x1022F000`）对 dabacon 的引用只有 `DFIND` ×19、`DRETUR` ×10、`GATREF` ×2、`XTREE` ×2、`CGETEL` ×2、
`CLIMBM` ×1、`CLIMBA` ×3、`NATTA` ×3；**`db_find_reference`（opcode 46）与 `DB_RefTableIterator` 一次都没有**——Core3D
别处导入了它们，联动规则表不用。~~**结论：core 的联动只有三种边——owner 链（上卷 / owner / 祖父 / 成员向下）、
正向引用数组（`CLIST` / `ELCXRF` / `REFGRD`）、结构连接（§3，正向读 + 回指核对）。反向引用一条都没有。**~~
**★0907 C6 收窄这句（§8.6）**：`change/` 模块**直接**没有反向引用调用，这一条不变；但 `GATREF(CLIST)` / `GATREF(ELCXRF)`
读的是伪属性，它们的实现（Core3D `CAB_DB_PseudoLALCAB::takeCables`、core.dll `DB_PseudoBACKRE`）就是 `DB_RefTableIterator`。
所以 core 的联动是三种边加一条尾巴——owner 链、正向引用数组（只剩 `REFGRD` 一个是真正向）、结构连接，**外加 E/F 两条
经伪属性走引用表的反向边，源限 `CWBRAN` 与 `EQUI`/`ELCONN`、属性限 `STAREF`/`ENDREF`、目标是电缆**。

对 gen-model：#15 孔洞是 owner 子树内的成员 → 整根重生成**构造性覆盖**，不再是「语料上不可达」；C3「通用反向级联」
在与 core 对齐这件事上~~没有对应物~~ **只有 E/F 这两条电缆边一个对应物（§8.6，归 C8 提案）**，不是通用反向级联
（它仍可以是我们比 core 多做的事，但通用那一半不是缺口）。

### 8.2 `RECON0` 不查表：沿 `CREF` / `JOIS` / `JOIE` 导航，然后**写回**两端

`connect/RECON0(mode ∈ {RECO 829026, DISC 604588}, end ∈ {JOIS 912394, JOIE 636832, BOTH 703892}, a4, a5)`（`0x1025F521`）：

```
el = DGETF(REF); type = DGETI(TYPE)
type ∈ {PJOI, SJOI, SUBJ, FIXING}:                        ← 节点侧
    cref = DGETF(CREF); DSAVE; DGOTO(cref)                  ← 去对端型钢
    end  = JOIS 若 EQREF(DGETF(JOIS), el)，否则 JOIE 若 EQREF(DGETF(JOIE), el)，否则 NULL   ← 与 ADSTCH 同一双向核对
    mode == DISC → cref = NULREF
    STRCNT(type(cref), cref, end, el, a4, a5, &err); DRESTO
type ∈ {SCTN, GENSEC}:                                     ← 型钢侧
    end ∈ {BOTH, JOIS} 且 mode == RECO → STRCNT(err, el, JOIS, DGETF(JOIS), …)
    end ∈ {BOTH, JOIE} 且 mode == RECO → STRCNT(…, el, JOIE, DGETF(JOIE), …)
其它 noun 且 mode == RECO → connect/RECONX(a5)；否则 ERROR + LABORT
```

`connect/STRCNT`（`0x102616EA`）：任一端 NULREF = **断开**：去节点 `DRMUDA(JFRE)`，去型钢 `structures/SETREF(型钢, type, NULREF, JOIS|JOIE)`、
`BPUTI(CTYS|CTYE, NULL)`，DB 类型 (2, 25|26) / (41, 54) 时再 `structures/MPUTR`；否则去节点读 `TYPE`：`FIXING` → `connect/FXCONN`，
其它 → `connect/JTCONN`。

`connect/JTCONN`（`0x10261B5B`）写：型钢 `JOIS`/`JOIE` ← 节点（`SETREF`）、型钢 `CTYS`/`CTYE` ← 节点 `SPRE→CATR→CTYA`（`BPUTI`）；
节点 `JFRE`（`BPUTL`）；经 `REF` 写 `ZDIS` / `SZDI` / `EZDI`（`BPUTR`，`MPUTR`）；读 `CUTP` `CUTB` `JLIN` `DELP` `NPOS` 算几何。
`connect/FXCONN`（`0x1026358C`）写：型钢 `JOIS`/`JOIE`、`CTYS`/`CTYE` 同上；**FIXING 的 `POS`**（`BPUTRA`）吸附到型钢；经 `REF` 写 `ZDIS`。

对 C2 的意义：core 的结构联动不只是重画，还**把连接属性写回两端**——这些写在保存的会话里就是账本条目。所以真实会话里
「改节点 → 型钢也进账本」是常态；0907 普查里 12 个「只改一端」的窗口，是写回没有改动型钢字节的那类（型钢的 `JOIS`/`CTYS`
本来就是那个值）。C2 补的正是这一类；不需要也不该去模拟这些写。

### 8.3 `DB_Element::hashValue()` = hard type 的 noun 词 hash

core.dll `0x593dd00`：`noun = this+12`（空或 `NOUN_UNKNOWN` 或 UDET 构建号过期 → `setType`）→ `DB_Blob::getContentType(noun)` →
`DB_Noun::hardType(...)` → `DB_Noun::hashValue`（`0x58d9860`，就是 `DB_Noun+92` 那一格）。所以 `EVALAT(a2, &hashValue(el), attr)`
里 `*a3` 与 `SCTN` = 821683 这类常量比，比的是 **noun 词 hash**；UDET 元素按其 hard type 走规则表（`:MYSCTN` 当 `SCTN`）。

### 8.4 两个「未解 noun」是死分支

`DB_FortranInterface::stringToHashValue`（`0x58dc4e0`）取前 6 字符、空格补齐 → `ENHAS2`：`hash = 531441 + Σ code(c_i) · 27^i`，
`code(A..Z) = 1..26`、`code(' ') = 0`、其它字符 → hash = 0。字母表只有 27 个符号，所以 `< 387951930` 的值**唯一**解回一个
≤ 6 字母的词：`271245643` = `YASJWR`、`269526523` = `VWIDTR`。二者都不在 3.1 字典（`noun_caps.json` 1931 个）里，也不可能是
UDA / UDET（`hashValueToString` 用 `a2 > 387951929` 区分那一族）。它们各自嵌在规则 A（楼板 / 墙洞 / 栏杆 / 辅助图元族）
与规则 E（`CWBRAN` 旁）的常量列表里——**旧产品线留下的 noun，3.1 数据永远命不中**，规则表因此少两个 noun、不多不少。

### 8.5 `UPDATN` / `UPDATD` 之后到 3D 画列表的那一跳

```
UPDATN (0x1022E3E7) / UPDATD (0x1022E5AC):
    UQGRAF() 图形开着 且 !UQUCUR()（不在 UCB 里）:
        [UPDATD: MPRCSS; USCHGO(a2)]
        sub_1050EA70()  =  DES_DrawListManager::instance()->updateGraphics()          ← 那一跳
            updateGraphics (0x1052D330):
                UpdateChangeList (0x1052B3C0): change/QCHGLS 句柄（sub_1022C3D7）→ HQLNIR 取长度 → for i = 1; i += 3: HGETIA
                                               → DB_Ref → DB_Element → sub_10485C20 收进容器 → 排成有序集 → 逐个 sub_1022E020 = EVALCD(ref, &类别)
                                               （形状像「去重后按类别重灌」，这一步只看了调用序、未细读）
                每个画列表: gotoDrawList → change/UPGRPH (0x1022EAEE) ；全局或 ARF 列表再 change/UPCACP (0x1022FCD0)
                promoteElement / isInDrawList 维护顶层件所属列表
        HLENIR(QCHGLS, 0)                                                              ← 清队列
    FDBUPD(); sub_1023190B() 的两个返回值 == 1 或 == 6（体未读）时: sub_10507C60() = VWUPDT、sub_10507C30() = VWCLRF   ← 视图层刷新（MTR_Entry 名）
    UPDATD 另有: dword_10E98500（IDCHNG 置的 INTUBE 旗标）→ PLCDEL()；末尾 DLL_draft::updcps()
```

`sub_10507C30` / `sub_1050EA70` 就此坐实：前者是 Fortran 可调的 `VWCLRF` 包装（`sub_1052DDC0` → `sub_1052DB20`），后者是
`DES_DrawListManager::updateGraphics()`。09-04 清单 §2.3 / 核对表 R5 说的「QCHGLS 步长 3、UPDATN 消费」在 `UpdateChangeList`
里逐字可见（`for (i = 1; i < len; i += 3) HGETIA`）。

### 8.6 C6：`CLIST` / `ELCXRF` 都是伪属性，底下是引用表反向查——0906 把 E/F 两条的属性写反了

**先改错。** 0906 §2.2 写「E: `CWBRAN` → `GATREF(ELCXRF)`、F: `EQUI`/`ELCONN` → `GATREF(CLIST)`」。0907 直读 `EVALAT`（`0x1022C679`）
伪码第 155–178 行与两处 `GATREF` 的属性常量：

```
if (*a3 == 202303956 /*CWBRAN*/ || *a3 == 269526523 /*VWIDTR 死 noun*/)  GATREF(a2, a3, &unk_10B23C98, &1, &2000, &0, out, &n, &err)
if (*a3 == 724361    /*EQUI*/   || *a3 == 209154074 /*ELCONN*/)          GATREF(a2, a3, &unk_10B23CA0, &1, &2000, &0, out, &n, &err)
unk_10B23C98 = 11541126 = db1_dehash → "CLIST"      unk_10B23CA0 = 96665729 = db1_dehash → "ELCXRF"
```

所以 **E = `CWBRAN` → `CLIST`，F = `EQUI`/`ELCONN` → `ELCXRF`**。两处的 `bin_search` 佐证：Core3D 全库 `86 1A B0 00`（`CLIST` hash）只出现在
`0x10B23C98` 这一处；`81 00 C3 05`（`ELCXRF` hash）只出现在 `0x10B23CA0`；core.dll 两个立即数都是零命中（它的 `.vlizer` 段有一处 `ELCXRF` 字节序列，
不是代码常量）。

**`CLIST` 是伪属性，提供者全在 Core3D 的电缆 / MDR 模块，没有一个挂在 `EQUI` / `ELCONN` 上。**

- core.dll 静态初始化 `sub_566FB10`（`0x566fb74`）：`ATT_CLIST = new DB_Attribute("CLIST", true)`。`DB_Attribute::isPseudo`（`0x58d2570`）看的是
  字典解出的类型码 `∈ {4, 5}`，不是这个 bool。它不在任何 noun 的存储布局里：`all_attr_info.json`（1878 noun）`"name":"CLIST"` 零命中，
  `ELCONN`（27 个属性）与 `EQUI`（50 个）的属性表里都没有它；core.dll 里没有 `0xB01A86` 立即数——dabacon 那一侧不认识它。
- 注册点两处，都在 Core3D：
  - `sub_1096A9D0`（← `sub_10987470`，电缆模块 plug 注册，66 次 `DB_PseudoAttPlugger::instance`）第 481 行：
    `instance(ATT_CLIST, NOUN_CWBRAN, app::cab::CAB_DB_BranchCableList_pseudo)` → `addGetRefArrayPlug`。
  - `sub_109B7270`（← `sub_109B61A0` ← `GFXINI`，186 次注册）：`POINTR` → `CAB_DB_PseudoCLISTonPOINTR`；`HATTA` / `TATTA` →
    `MDR_DB_PseudoCLISTonHATTA_TATTA`；`RATTA` / `RNODE` → `CAB_DB_PseudoCLISTforRATTA`；`CNODE` → `CAB_DB_PseudoCLISTonCNODE`；`PULLN` →
    `CAB_DB_PseudoCLISTonPULLN`；`CTSTRA` `CTCOUP` `CTRISE` `CTBEND` `CTTEE` `CTCROS` `CTREDU` → `MDR_PseudoCLISTforTrayElement`。
  - 安装目录里引用 `?ATT_CLIST@@3QBVDB_Attribute@@B` 导出的 DLL 只有 core.dll 与 Core3D.dll，含 `PseudoCLIST` 字样的类也只在 Core3D
    （`D:\AVEVA\Everything3D2.10` 全目录二进制扫描，Core3D.dll 2022-09-20 版同样是这六个类；本机 `Everything3D3.1` 目录今天只剩
    core.dll 及副本 + libgeom / libgm，IDB 的源文件 core3d.dll 已不在原位，所以全目录那一步只能在 2.10 上做）。Core3D 自己的电缆代码 `sub_1096BF30`（对一批元素逐条发 WARNING 125「连着的电缆」）对
    `POINTR` / `CWBRAN` 读 `CLIST`、对其它 noun 读 `LALCAB`，**对 `EQUI` / `ELCONN` 读的是 `CONNEC` + `members(ELCONN)`**——它自己也不认为
    `EQUI`/`ELCONN` 有 `CLIST`。
- 算法（伪码 + 属性导入）：
  - `CAB_DB_BranchCableList_pseudo::getAtt`（`0x10966D50`）：`sub_10963F20(el, HATTA|TATTA ∪ RNODE|RATTA 谓词)` 收分支下的路由节点 →
    对每个读 `CLIST` 与 `LALCAB` 并集 → 排序去重 → `sub_109C9090(qualifier)` 按电缆状态 `CBSTA` 过滤。
  - `MDR_DB_PseudoCLISTonHATTA_TATTA::getAtt`（`0x109C41B0`）= `LALCAB` + 同一过滤；`CAB_DB_PseudoCLISTforRATTA`（`0x109BC310`）与
    `CAB_DB_PseudoCLISTonPOINTR`（`0x109BCA30`）读 `CONATS` / `LALCAB` / `SLRAP` / `SLRBP` / `POSREF` 再并。
  - **`CAB_DB_PseudoLALCAB::getAtt`（`0x109C0310`）→ `takeCables`（`0x109C96B0`）：`DB_MDB::dbs(attr)` 逐库
    `DB_RefTableIterator(db, STAREF | ENDREF, el)`，对每条 `element()` 取 `owner()` 两级、按 `type` 收**——就是「`STAREF`/`ENDREF` 指向这个
    路由节点的 `EXTGEO` 段所属的 `CABLE`」。反向、跨库。
  - `CAB_DB_PseudoCLISTonCNODE::getAtt`（`0x109BC840`）= `unique(sort(ATCONN))`，`ATCONN` 自己也是伪属性（`MDR_DB_PseudoATCONNonCNODE`）。

**`ELCXRF` 也是伪属性，在 core.dll 里，就是 `BACKRE` 加两个属性限定。**

```
core.dll ??_7DB_PseudoELCXRF_DESI@@6B@ = 0x5d66a3c ; vt[0] = getAtt = 0x5A684E0:
    q = DB_Qualifier(); q.setQualifier(ATT_ENDREF); q.setQualifier(ATT_STAREF)
    return DB_Element::getAtt(ATT_BACKRE, q, out)
DB_PseudoBACKRE::getAtt (0x5A5BC20):
    attrs = qualifier 里的属性（空则取 DB_Attribute::getAllAttributes 里 type == 5 && isTable 的全部）
    for attr in attrs: for db in DB_MDB::instance()->dbs(attr):
        for it = DB_RefTableIterator(db, attr, el, el); !it.isDone(); it.next(): out.push(it.element())
注册 sub_5A68B40（← sub_5A41330 ← sub_51DAADC）：instance(ATT_ELCXRF, NOUN_x, DB_PseudoELCXRF_DESI) → addGetRefArrayPlug，x ∈
    { EQUI ELCONN SUBE ELFITT VALV VTWA VFWA INST PCOM AHU DAMP HFAN SILE BATT IDAM TP PLEN HACC
      SCEQUI SCELCO SCSUBE SCACTU SCFITT SCHVFI SCINST SCOINS SCVALV }（EVALAT 只对 EQUI / ELCONN 用它）
```

`STAREF` / `ENDREF` 在 `all_attr_info.json` 里是 `CABLE` 的 schema ELEMENT 型存储属性（`89637670` / `89639681`；`EXTGEO` 也持有，普查边可见），
`BACKRE` 走的 `DB_RefTableIterator` 正是 dab-ref-table 证据里那张 kind-16 引用表。所以规则 F 展开就是：**`EQUI`/`ELCONN` 变了 → 引用表里
`STAREF`/`ENDREF` 指向它的元素（= 接在它上面的电缆）全部 REDRAW**；规则 E 展开是：**`CWBRAN` 变了 → 它路由节点被 `EXTGEO.STAREF`/`ENDREF`
引用的那些电缆全部 REDRAW**。两条都是反向边，都跨库。

**语料（486 个 DESI 库；实例数来自 `noun_census_probe` 0907 → `docs/evidence/2026-09-07-cable-noun-census.json`，边数来自 0907 普查
`docs/evidence/2026-09-07-cross-root-reference-census.json` 的 `all_edges`）：**

| 项 | 数 | 备注 |
|---|---|---|
| `ELCONN` 实例 | **1607** | 34 个库，全在 ams / aps 的电气 / 设备库里 |
| `CWBRAN` / `CWAY` 实例 | 1699 / 187 | |
| `CABLE` 实例 | **116** | 只在 9 个库：`aps250176/250179/250182` 及其副本 `250514/250517/250520`（23+16+10 ×2），`aps7200_0001` / `zdj7200_0001` / `zdj7210`（6 ×3） |
| 路由节点 `HATTA` / `TATTA` / `RATTA` / `RNODE` / `POINTR` / `CNODE` / `PULLN` | 1695 / 1695 / 349 / 2943 / 12926 / 3 / 0 | |
| `CABLE.STAREF → EQUI` / `CABLE.ENDREF → EQUI`（同库） | 18 / 18 | 全部跨根（`fwd_uncovered_nearest` = `rev_uncovered_nearest` = 18；`CABLE` 自己是根，`nearest_root_nouns: CABLE`），就是 zdj7200 那 6 根电缆 ×3 份拷贝 |
| `CABLE.STAREF → @desi…` / `CABLE.ENDREF → @desi…`（跨库） | 98 / 98 | 对端库 `250152/154/156/160/162/164` 及副本 `250528/530/532/534/536/538`——正好是语料里含 `ELCONN` 的那六个电气库；对端 noun 未逐条解 |
| `EXTGEO.STAREF/ENDREF → HATTA/TATTA/RATTA/RNODE`（同库） | 204 / 204 | 全部跨根（`CABLE` 根 vs `CWBRAN` 根）——规则 E 那一头 |
| `EXTGEO.STAREF/ENDREF → @desi250176/179/517/520`（跨库） | 76 / 76 | |

`CABLE` 与 `ELCONN` 在这套语料里**从不同库**（含 ELCONN 的 34 个库 CABLE = 0；含 CABLE 的 9 个库 ELCONN = 0），所以规则 F 在这里命中的
`ELCONN` 边全是跨库边，同库的 36 条对端是 `EQUI`。

**对 gen-model（清单 #13 / #14 的 E 条）：** ~~`CLIST` 不是存储属性、`ref_rev` 收不到~~ → 这是一条**反向边**，不是正向数组：
direct 文件窗口路径没有；旧 PE 路径的 `ref_rev` 按 schema ELEMENT 型收 `CABLE.STAREF`/`ENDREF`，电缆单元会级联（超集）。计划 §3 C6 的判词是
「无实例 → 语料上不可达；有 → 另开计划」——**有**：登记为 **C8 提案「电缆端点 / 路由反向级联」**（源 = `EQUI`/`ELCONN`（F）与 `CWBRAN`
的路由节点（E）；边 = `STAREF`/`ENDREF` 反向；目标 = `CABLE`（`EXTGEO` 归 owner `CABLE`）的根；跨库边占多数 → 要跨库的反向索引；
窗口未量）。今天电缆几何本身没按路由落地（`CABLE` 在 `stage3-noun-routing-gaps.md` §3b 里「现走 cata_model」；e3d-model 的路径段
只移植了 `POINTR` 直线段，`EXTGEO` 那支未接），所以这条像 #12 一样**几何上暂不显**，电缆几何落地即成正确性项。C3「通用反向级联」在 core 侧的对应物就只有这两条电缆边——不改 C3 不立项的判词。

**★0907 同日：用户拍板先做同库版 → 已接线**（`src/data_interface/cable_endpoint.rs`，`WindowRootSources.cable_endpoint`，开关
`model_cable_endpoint_cascade` 默认关）：账本里的 `EQUI` / `ELCONN`（F）查自己、`CWBRAN`（E）查自己 + T 端索引的直接成员，
`ReadOnlyEngine::referrers_of` 到 T 端文件自带的 `STAREF` / `ENDREF` 反向表点查，引用者 owner 链上的 `roots_T` 根并进 `touched`——就是
上面 `DB_PseudoBACKRE` / `takeCables` 那张表、那两个属性，只差「本库」这一层（core `DB_MDB::dbs` 逐库并）。`zdj7200_0001` 实测：表里有
`STAREF` / `ENDREF` 两张；假定 `EQUI 23584/10206` 变了 → 6 条边 / 6 个电缆根，与正向读 `CABLE.STAREF`/`ENDREF` 反推的集合相等；假定
8 条 `CWBRAN` 变了 → 70 次点查 / 27 条 `EXTGEO` 边 / 6 个根全是 `CABLE`；会话链 9 步 + `aps7200_0001` 3→4 开关两态 `regen` 逐字相同
（语料里没有「只改一端」的自然窗口：2→3 时会话 3 还没有这两张表，4→5 两端一起建）。
