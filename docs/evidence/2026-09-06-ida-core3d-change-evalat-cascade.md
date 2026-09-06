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
再按 noun 走专项规则——上卷到顶层可画件（`FNDTOP`）、取 owner / 祖父、**沿正向引用**（`GATREF` 读 `CLIST` /
`ELCXRF`）、**沿反向引用**（`DFIND FIRST/NEXT` = 「谁引用我」）、结构连接（`ADSTCH`：`CREF` ↔ `JOIS`/`JOIE`、
`CFRA` ↔ `PFRE` **双向核对**）、隐含管段（INTUBE → 前一个元件）——每命中一个目标就
`EVALCD(ref, 类别)` 推进 QCHGLS。

GETWORK 也走这条路：`sub_1052F2E0` 对每个变更元素先 `FNDTOP`（按 **`toplevel`** 位找顶层可画件），
再对「owner 不在集合里」的顶层件调 `EVALAT(top, noun, 331445106=REDRAW)`。所以 09-04 清单 §2.3
「GETWORK 全程没有任何依赖递归」**要改成**：过滤器自己没有，但它把每个顶层件交给 `EVALAT`，
`EVALAT` 里有引用规则（EQUI 的 `CLIST`、CWBRAN 的 `ELCXRF`、CTRAY / 网格连接点的反向引用、
设备子件与 PANE 的「谁引用我」）。

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
| `connect/RECON0` | Core3D | `0x1025F521` | — | ADSTCH 命中后的重连 |
| `desdblib/FNDTOP` | Core3D | `0x10380E38` | `0x21f` | 从 CE 沿 owner 上行，找第一个 `LISTOP` 为真的元素；顶层件的 owner 若是 `TMPL` 则改取 owner |
| `desdblib/FNDEQP` | Core3D | `0x1038174C` | `0x244` | 点类 noun（`LPOINT`）→ 所属几何件 |
| `nounlib/LISTOP` | **core.dll** | `0x561C93C` | — | 顶层可画件判据：读 noun 逻辑位 **`0xA187C`（`toplevel`）**；为假时退到 `0xA103E`（`primitive_a`）配特例（§4） |
| `INCOMP` / `IPCOMP` / `IHCOMP` / `IECOMP` / `LPOINT` | core.dll | `0x561C1B0` / `0x53A9A10` / `0x53A9AAC` / `0x561BC58` / `0x561CD78` | — | noun 谓词（管道 / 管道 / HVAC / 设备子件 / 点类），本轮**未读体** |
| `DFIND` / `GATREF` / `XTREE` / `CGETEL` / `CLIMBM` / `CLIMBA` / `NATTA` | core.dll（Core3D `.idata` 导入） | — | — | dabacon 导航原语：反向引用 FIRST/NEXT、取属性引用数组、子树遍历、按 noun 取祖先、按 noun 表爬升、前后元件 |
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
| A | `FLRLAY` `WLOPEN` `HRGATE` `RAIL` `KICKPL` `HRPOST` `BPOPEN` `AIDLIN` `AIDARC` `AIDCIR` `AIDPOI` `AIDTEX` + 1 个未解 noun（`271245643`） | `FNDTOP` | owner 链 | 顶层可画件 | 4 |
| B | `GRIDPL` `GRIDCY` `GRIDEL` `GRIDFA` | `CGETEL(REFGRD)` → `XTREE` 遍历该网格子树 | 正向引用 `REFGRD` + 子树 | 子树里 `GRIDPL` / `GRIDCY` 的 `REF`（先过 `sub_1050D810` 谓词，**未读**） | 4 |
| C | `CGRDCP` `CGRDLP`（网格连接点） | `DFIND FIRST/NEXT` | **反向引用**（谁引用我） | 引用者里 noun ∈ {`FPFITT` `ELFITT` `HVACFI` `INFITT`} 的 | 继承 |
| D | `CTRAY` | `DFIND FIRST/NEXT` | 反向引用 | **全部**引用者 | 4 |
| E | `CWBRAN` + 1 个未解 noun（`269526523`） | `GATREF(ELCXRF)` | 正向引用数组 `ELCXRF`（≤2000） | 数组里每个元素 | 4 |
| F | `EQUI` `ELCONN` | `GATREF(CLIST)` | 正向引用数组 **`CLIST`** | 数组里每个元素 | 4 |
| G | `CLNPNG` | `DFIND` 两层 | 反向引用 ×2 | 引用者全部；引用者若是 `CLNCGR`，再标它的 `CLNTIL` 引用者 | 继承 |
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
3. 否则（**T 是设备子件或 PANE**）→ `DFIND FIRST/NEXT` 遍历**所有引用 T 的元素**，其中 `INCOMP`（`HOLC`
   **孔洞 / 负实体构件**）为真的逐个标记——即「**被打孔的板 / 设备子件动了，引用它的孔洞构件跟着重算**」；
   随后 `T` 自身也标记。（第一版本文把这条读成「接在 NOZZ 上的管道元件」，**错了**：`INCOMP` 读的是 `HOLC`
   不是 `PIPC`，见 §2.2a。管道连接在 core 里不走这条——`HPOS`/`TPOS` 是存储属性，09-04 清单 §2.9。）

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
| 新 #15 孔洞 / 负实体 | §2.3 第 3 条：PANE / 设备子件（`EQUC`）变了 → `DFIND` 找**引用它的孔洞构件**（`HOLC`）逐个重算；规则 J / P：孔洞构件自己变了 → 自身 + owner | 负实体在我们这边是 owner 子树内的成员（整根重生成覆盖 owner 内的孔）；**跨 owner 的孔洞引用**（孔洞元素引用另一个 owner 下的板）靠 `ref_rev`：旧路径有、direct 无 | ❓ 与 #12 同一类：direct 路径缺反向级联；真库里有没有跨 owner 的孔洞引用未查 |
| #4 CATA → DESI | **无**：`EVALAT` 里没有一条走 `SPRE`/`CATR`；GETWORK 库过滤只放 DESI/ISOD | `ref_rev` / 指纹失配 | 09-04 结论不变：我们比 core 强，无对照物 |
| #9 UDA / #10 XRPNTR | **不在 EVALAT 里**（那两条在 `DB_UserChanges::attributeModified`，ADR-061） | 不做 | 不变 |
| **新 #12 结构连接** | §3 `ADSTCH`：`CREF` ↔ `JOIS`/`JOIE`、`CFRA` ↔ `PFRE` 双向核对后联动对端。注意它是**从变更元素出发的正向读**（读 `el.CREF` 到 X，再读 `X.JOIS` 核对），**不需要反向表** | **未登记**。旧 PE 路径：`manual_update.rs::reference_edge_eligible` 按 schema `att_type == ELEMENT` 收边，`CREF`(35 noun) / `JOIS` / `JOIE`(4) / `CFRA`(3) / `PFRE`(1) 都是 ELEMENT 型 → 进 `ref_rev`，对端的交付单元会级联（不做双向核对 = 超集，多做不漏）。**direct 文件窗口路径：没有任何反向级联**（09-04 清单 #4 同一个洞：`direct_store::referrers_of` 零调用方） | ❓ **direct 路径上是真缺口候选**，且与 #4 不同：ADSTCH 只要正向读，direct 规划器给结构 noun 加一步「读 `CREF`/`JOIS`/`JOIE`/`CFRA` 的目标并入根集」就能对齐，不必等反向表。验收窗口：改一根 SCTN 的 `POS`，对端 SCTN / 节点是否进重算集 |
| 新 #13 设备连接表 | 规则 F：`EQUI` / `ELCONN` 变了 → `CLIST` 数组里的元素全部 REDRAW | `CLIST` **不在** `all_attr_info.json` 的存储属性里（`ELCXRF` / `REFGRD` 也不在）——多半是伪属性，`ref_rev` 收不到 | ❓ 待核（电缆 / 连接件语料里才有内容）；先确认 `CLIST` 是不是伪属性、由谁算 |
| 新 #14 桥架 / 网格 | 规则 B–E、G（`ELCXRF` 正向、`CTRAY` / 网格连接点反向） | 桥架族 `gb==3`（09-04 §2.5），几何覆盖度问题 | 登记不排期 |
| 顶层单位 | §4 `toplevel` | 生成根按 `significant` | 不改判据（核对表 R0-3），但 3D 重画单位 = TOPF 这条要写进核对表 W4.1 的结论 |

**最值钱的一条是 #12**：它是 core 明确做了、而我们的十一类表上根本没有的一条边，且属于 DESI 三维模型增量的范围。

## 7. 不许当已证

- §2 / §3 的规则来自 Hex-Rays 伪码 + 常量解码。这批函数是 Fortran 风格的平铺调用（`DGETF` / `DFIND` / `GATREF`
  全走导入表，无 `getField` 出参陷阱），伪码里**没有**出现 §1.4 那类孤零零的 `getField(v); return 0` 形状，
  所以采信；但**没有逐指令核**。`sub_1052F2E0` 的插入点（§5.1）是逐指令核过的。
- `DB_Element::hashValue(el)` 被当作「noun hash」用（`EVALAT` 的 `a3` 与 noun 常量比较）。这是从比较对象反推的，
  **没有读 `hashValue` 的实现**。
- `DFIND(FIRST/NEXT, 1)` 读作「反向引用：引用 CE 的第一个 / 下一个元素」，依据是 hash 解出的字面
  `FIRST` / `NEXT` 与循环形状（每轮 `DRETUR()` 回到 CE）。`DFIND` 本体 = dabacon **opcode 43**（core.dll
  `0x5aaff40` → `sub_5ACFF70(43, …)`；名表里 45 = `DFINDF`），与引用表搜索 326/328 不是同一原语——它查哪张表
  **未读**。
- ~~五个谓词未读体~~ 已读（§2.2a）；`sub_1050D810` 已读（= `DES_DrawList::exists`）；`CLIMBM` 表已解
  （{PANE, GENSEC, SCTN, BPANEL}，`0x10D906D0` = 计数 4）。
- 两个 noun 常量 `271245643`、`269526523` 在 `noun_caps.json` 里没有、逆 hash 也不是词（`YASJWR` / `VWIDTR`），
  可能是 UDA 或别的编码，未解。
- 类别语义 `1` = INTUBE、`4` = REDRAW 沿用 ADR-002 的静态结论；`2` 与 `3`（位姿）按
  `src/data_interface/dchc_change_classes.json` 的口径，本轮未从二进制重证。
- `docs/evidence/2026-09-06-dab-ref-table-write-path.md` 说 dab 引用表只覆盖 kind-16 属性、`HREF`/`TREF`/`CREF` **无表**；
  而本文 §2.3 第 3 条的 `DFIND FIRST/NEXT` 却能找到「引用 NOZZ 的管道元件」（`HREF`/`TREF`）。两者只有一个对：
  要么 `DFIND` 走的不是那张表（会话内内存反向表 / 扫描），要么 kind-16 那条结论不完整。**本轮没有读 `DFIND`，
  这条矛盾留给 dab 引用表那条线。** ADSTCH 不受影响（它是正向读）。
- **没有跑任何一条联动的实测**。§6 的 ❓ 三行是取证结论，不是缺口结论。
