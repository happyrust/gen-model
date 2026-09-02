# Plannator 开发计划：e3d-model NOUN 覆盖收口 —— 把「哪些 noun 没被考虑到」变成一张可机械复算的表

> 计划 ID：`2026-08-31-noun-coverage-closure`
> 创建：2026-08-31
> 状态：**draft**（仍未过 plannotator 门禁）；阶段 **N0–N4 全部 done**（2026-08-31）。
> 残留一项：N3 与既有计划 §2.2 builder 表未对账。
> 换版本引入的 `CTRAY` 记账不自洽已于 2026-08-31 改判闭合（见 N4 段末）。
> N2 的 `NOZZ`/`EQUCOM` 半边证据已于 2026-08-31 用 APS 工厂语料补齐闭合（见 N2 段末）。
> 级联外 14 个未知 noun 已于 2026-08-31 逐个定性完毕（见 N2 段末 + `docs/noun-coverage-matrix.md` §九），
> 但定性过程查出**两笔本计划范围之外的新账**，需另开条目：
> ① `TMPL` 判 `Unknown` 不消费成员，`aps250110` 实跑有 **1559 件负体成孤儿**，孔洞静默丢失；
> ② `HANDRA`/`RAIL`/`KICKPL`/`STRLNG` 与 `HRPOST` 等同属 `IASLCO` 总成族却被字典
> `IPFCOM` 位劈成两半，是**真欠账**，不是「级联外所以不算」。
>
> 拍板前提（用户 2026-08-31 15:55 原话）：
> 「分析 e3d-model，结合 ida-bridge 分析当前的模型生成算法是否已经覆盖完毕了，
> 针对不同 NOUN 的模型生成处理，是否都考虑到了。帮我总结，使用 plannator。」
>
> 权威（按证据强度排序）：
> 1. **dabacon 字典实读**——core.dll 自己读的就是这份，字段号由本轮活桥从 `I*COM`
>    反编译逐个解出，**不是手抄表**。计划创建时用的是
>    `D:\AVEVA\Everything3D2.10\attlib.dat`（1384 noun）；**N4 已换成 E3D 3.1 的
>    `E:\reverse\e3d\shadow_e3d31_aps_all\attlib.dat`（5 840 896 bytes / 1931 noun），
>    当前全部数字以 3.1 为准**。
> 2. **ida-bridge 活体反编译 `core.dll`**（`idalib-41236` → `D:\ida_scratch\plant3\core.dll.i64`）。
>    ★ 本轮关键变化：前几轮只有 `Core3D.dll.i64`，`I*COM` 谓词只能看到 `__imp_` thunk，
>    所以既有计划把它列为「静态定不死的低优先残留」；本轮 core.dll 已挂上，**该残留结掉**。
> 3. 既有计划 `.planning/2026-08-31-core-aligned-model-generation/task_plan.md`（approved）。
> 4. `vendor/e3d-model` 源码实读（本轮只读不改）。
>
> 上下文：`上下文/会话-2026-08-31-noun覆盖审计-7KG8.md`（实时更新）

## 与既有计划的关系

`2026-08-31-core-aligned-model-generation`（**approved**）是**算法线**：按 core.dll 的 CSG
架构把生成算法另立一套，阶段 P0→P5。它的 P1 就叫「分类表对齐 core.dll 注册表」，
验收写的是「`unknown_nouns` 在 ams1112 上为空或每条有归因」。

**本计划不另起炉灶，是给那条 P1 补一个它当时拿不到的东西：判据。**

| | 算法线（0831 core-aligned） | 本计划 |
|---|---|---|
| P1 的判据 | 「逐条对齐 §2.3 注册表 + §2.4 plug 名单」——那是 **Core3D 现代路**的名单，只有 12 个图元 + 60 个 plug noun | 全库 1384 个 noun 的**旧路**家族位，机械导出，277 个命中级联 |
| 覆盖率怎么算 | 拿 ams1112 跑一遍看 `unknown_nouns` 空不空 | 不依赖语料：字典说该建的 277 个，逐个对 `classify()` |
| 关系 | 本计划**产出**它 P1 的输入表与回归夹具 | 本计划**不改**算法方向，不动 P2–P5 |

**结论：本计划是 P1 的前置与判据供给，不替代它。** 既有计划文档本轮**不改**（避免与在飞
agent 撞），需要回填的三处修正列在 §7.1，等门禁后按拍板意见一次性回写。

---

## 一、先回答问题

**没有覆盖完。按 core.dll 自己的字典口径，277 个 noun 该出几何，e3d-model 现在能建 22 个；
另有 129 个连分类都没有（落 `Unknown`），126 个有名目但本期不建。**

```
core.dll GTGEOM 级联判定「该出几何」的 noun          277
├─ e3d-model 会建几何                                 22   ← 8.0%
├─ e3d-model 记账不建（Catalog/RouteMember/… 有名目）  126   ← 45.5%
└─ e3d-model 分类表欠账（落 Unknown，无名目）          129   ← 46.6%  ★
```

> **本节四数是 N0 当时的快照，不再回改**（改了就看不出各阶段吃掉了多少）。
> 随时可用 `cargo run --bin dump_categories` 接 `scripts/noun_coverage_report.py` 复算当前值。
>
> | 时点 | 字典 | 分母 | 会建 | 记账不建 | 欠账 |
> |---|---|---:|---:|---:|---:|
> | N0（本节快照） | E3D 2.10 | 277 | 22 | 126 | **129** |
> | N1 收工 | E3D 2.10 | 277 | 27 | 172 | **78** |
> | N2–N4 收工（当前） | **E3D 3.1** | **327** | **27** | **300** | **0** |
>
> 分母从 277 跳到 327 是 N4 换字典的结果（2.10 的 1384 noun → 3.1 的 1931 noun），
> **不是覆盖面变宽**。会建的仍是 27 个，几何覆盖率 27/327 = **8.3%**。
> 终态全表见 `vendor/e3d-model/docs/noun-coverage-matrix.md`。

**但「129 个欠账」不等于「129 个缺陷」，这一点必须说清楚，否则会导向错误的排期。**
拆开看是三类，处置方式完全不同：

| 类 | 数量 | 性质 | 处置 |
|---|---|---|---|
| **A. 真缺陷** | 4 类（下详 §4） | 分类表写错了 / 漏了兄弟 noun / 与 core.dll 直接冲突 | **必须改**，成本低，本计划 N1 |
| **B. 整族未建概念** | 42 + 14 + 42 | 几何集族、电缆桥架族、建筑装配族——e3d-model 里连这个类别都不存在 | 先**记账**（给它们一个类别名），建几何排到算法线 P3+ |
| **C. 语料外** | 其余 | ams1112/ams8000 里一件都没有，属别的专业线 | 记账即可，写明「有意不做」 |

**这条区分本身就是本计划最主要的产出**：现在的 `unknown_nouns` 把 A/B/C 三类混在一本账里，
一行 `{"CTBEND": 12}` 看不出到底是「写错了」还是「有意不做」。而 e3d-model 自己的宪法写着
「静默缺件是最高级别缺陷」——**一本分不出轻重的账，和没有账的差距没有想象中大。**

### 1.1 「覆盖完毕」这个问题为什么以前答不了

既有计划 §2.3/§2.4 的对标基准是 **Core3D.dll 的现代 CSG 注册表**：12 个 `CSG_Basic*`
图元类 + 12 个 `addPlug` 注册点上的约 60 个 noun。但 §2.4.1 自己已经坐实：
**ams1112 的主力 noun 在现代路上一个都没有，全走旧路。** 于是出现一个断层——

> 拿现代路的名单去审旧路的覆盖率，分母就是错的。

旧路的分派在 `GTGEOM`(0x10341d2e) 的 `I*COM` 谓词级联里，而 `I*COM` 的真身在
`core.dll`（FORTRAN），Core3D.dll 里只有 `__imp_` thunk。既有计划因此写道：
「exact I\*COM 谓词拿不到，列为低优先残留」「§6.4 的 NOUN→码表暂挂」。

**本轮 `core.dll.i64` 已经挂在活桥上，这两条残留一起结掉了**，分母才第一次算得出来。

---

## 二、底账是怎么来的（三步，每步都可复算）

不是读代码猜的，是从 core.dll 一路推到字典的机械推导。任何一步都能独立重跑。

### 2.1 第一步：从 core.dll 解出每个 `I*COM` 谓词读哪个字典字段

```powershell
& "C:\Users\dpc\.agents\tools\ida-bridge\.venv\Scripts\ida-bridge.exe" `
    exec idalib-41236 -f .ida_scratch/probes/icom_field_ids.py
```

22 个谓词形状统一：

```c
int __cdecl IPCOMP(int a1) {                      // 0x53a9a10
  ATNINT(a1, &unk_5D6785C, &v3, &v4);             // 读 noun 字典的整数字段
  if (!v4) v2 = v3;                               // 无错则取值
  return v2;
}
```

`&unk_5D6785C` 处的 dword 就是字段号。全表：

| 谓词（在 GTGEOM 级联内） | 地址 | 字段号 | 命中 noun 数 | GTGEOM 去向 |
|---|---|---|---|---|
| `IPCOMP` | 0x53a9a10 | 602413 | 58 | `sub_10714FC0` 复合几何 |
| `IHCOMP` | 0x53a9aac | 595979 | 30 | `sub_10714FC0` |
| `IFCOMP` | 0x53a9974 | 600459 | 32 | `sub_10714FC0` |
| `IPFCOM` | 0x561c2fc | 603790 | 52 | `sub_10714FC0` |
| `IECOMP` | 0x561bc58 | 606263 | 32 | `sub_10714FC0` |
| `ICABCO` | 0x561ba84 | 591978 | 15 | `sub_10714FC0` |
| `INCOMP` | 0x561c1b0 | 599651 | 12 | `sub_10714FC0` |
| `IG2COM` | 0x561bd90 | 605428 | 5 | `CGTCT2` + `sub_10714FC0` |
| `IGMCOM` | 0x561be2c | 604699 | 21 | `CGTCT2` + `sub_10714FC0` |
| `INGCOM` | 0x561c260 | 600170 | 16 | `CGTCT2` + `sub_10714FC0`（负几何） |
| `ICCOMP` | 0x561bb20 | 605100 | 4 | `CGTCT2` + `sub_10714FC0` |

不在级联内的 8 个（**记下来，是范围边界**）：`IHLCOM`(599813, **85 个船体 noun**)、
`IPTCOM`(604897, 9)、`IPLCOM`(601036, 2)、`ICOCOM`/`IFICOM`/`IJOCOM`/`IPRCOM`(各 1)、
`IHVCOM`(591821, 2.10 字典里查无此字段)。
`IASLCO`/`IUPCOM` 不读字段；`ISUBCO` 走 `DGETI` 读元素属性而非 noun 字段。

> **★ 船体 85 个 noun 不走 GTGEOM。** 这条把范围划死了：`H*` 那一大片
> （HPLATE/HSTIFF/HPILLR/…）不在本条几何路上，`unknown` 里出现它们**不算**本线欠账。

### 2.2 第二步：把这 19 个字段从 dabacon 字典读出来，按 noun 出全表

```powershell
python .scratch/noun_family_probe.py      # → output/noun_family_matrix.json（1384 行）
```

读法直接复用 `gm_noun_caps_probe.py` 的 `AttrDataFile`（ATTOPE/ATGTIX/ATNLOG 页链 +
`basetype` 继承回溯），**与 core.dll 走的是同一份 `attlib.dat`、同一套字段号**。

产出每个 noun 一行：家族位集合、`primitive`/`geomset`/`extrusion` 能力位、
`graphicsBehaviour`、`positiveEquivalent`。

### 2.3 第三步：与 `classify()` 逐条对账

```powershell
cd vendor/e3d-model
cargo run --bin dump_categories -- data/noun-family-matrix.json out/noun-categories.json
python scripts/noun_coverage_report.py
```

判词直接来自 crate 里真正的 `classify()`：`dump_categories` 链接 `e3d_model::category`，
对家族矩阵里 1384 个 noun 逐个调一遍，把结果连同家族位写成 JSON；报告脚本只是把这份
产物摊开，自己不认识任何 noun。

> 初版审计曾把 `category.rs` 的 match 臂誊写进 Python 做对照，那是全流程唯一的手工
> 环节、也是唯一不会报错的失效点（分类表一改，影子表悄悄过期）。**N0 已经把它换成
> 上面这条链路并删除影子表**，见 §6 N0。下文所有数字均由该链路复算得出。

### 2.4 顺带结掉的既有残留：NOUN → db1 数字码表（既有计划 §6.4）

既有计划记的是「`DB_Noun` 静态镜像全 `0xffffffff`，noun→码由字典运行时装载，
两个 DLL 静态都读不到」——**结论没错，但结错了对象**：码不在 DLL 里，
它就在 `noun_flags.json` 的 `noun_hash` 字段里，双向可查。

把既有计划里所有硬编码码反查一遍：

| 出处 | 码 | noun |
|---|---|---|
| GTGEOM `IPCOMP\|IHCOMP` 分支 | 1062572 / 209154074 / 195113669 | **NOZZ / ELCONN / EQUCOM** |
| IFCOMP 分支附加 `sub_10018CE7` | 239044746 | **CTSUPP** |
| `sub_10343B80` 设计图元 switch | 207879217 | **GRIDLN** |
| 同上（码组） | 779182 / 560322 / 959395 / 790729 / 832210 / 813985 / 719964 / 856622 / 822719 / 985369 | **PVOL / RPLA / DATU / GRDM / POGO / POIN / IPOI / TANP / BOUN / DRAW** |
| `sub_10714FC0` v11=6 分支 | 644263 / 919825 | **PTSE / PTSS** |
| `cachelib/GTTUBE` | 808220 / 137403155 / 537123 | BRAN / TRUNNI / LUG（与既有结论一致 ✅） |

**两条立刻可用的推论：**

1. `NOZZ`/`ELCONN` 被 GTGEOM **硬编码**送进复合几何路，而 e3d-model 判它们
   `NonGraphic`（不建、不下钻）。这是**直接冲突**，见 §4 缺陷 5。
2. `sub_10343B80` 的 case 表里有 `POGO`/`POIN`/`DRAW`/`TANP`/`PVOL` 等 11 个 noun，
   e3d-model 一个都没认领（`POIN` 记成 ProfileData）。

### 2.5 顺带结掉的第二条：正负配对不必手写，字典里有

字典字段 `positiveEquivalent`(778791) 给出 12 对，与 Core3D `primList_` 的 12 个几何类一一对应：

```
NBOX→BOX   NCON→CONE   NCTO→CTOR   NCYL→CYLI   NDIS→DISH   NPOLYH→POLYHE
NPYR→PYRA  NREV→EXTR   NRTO→RTOR   NSLC→SLCY   NSNO→SNOU   NXTR→EXTR
```

> ⚠️ **`NREV → EXTR` 与 Core3D `primList_` 的 `CSG_BasicREV(REVO/NREV)` 配对冲突。**
> 字典说 NREV 的正体等价是 EXTR，注册表说 NREV 和 REVO 共用 `CSG_BasicREV`。
> 两者可以并存（「等价 noun」与「共用几何 builder」不是一回事），但**在查清之前
> 不许拿 `positiveEquivalent` 去驱动回转体的建法**。列 §7.2 查证项。

---

## 三、覆盖矩阵（277 个级联 noun × e3d-model 类别）

> **本节是 N0 时的快照（E3D 2.10 / 277 个），保留不回改。**
> N2–N4 收工后的终态（E3D 3.1 / 327 个 / 19 个家族逐个写终态）见
> **`vendor/e3d-model/docs/noun-coverage-matrix.md`**——那份是机械导出的，本节是历史。
> 下面 §3.1 那 129 个欠账名单**现已全部认领，`Unknown` 清零**。

| e3d-model 类别 | 级联内数量 | 终态 |
|---|---|---|
| `LoopExtrusion` | 6 | ✅ 会建几何 |
| `Primitive` | 5 | ✅ 会建几何 |
| `NegPrimitive` | 5 | ✅ 会建几何 |
| `Polyhedron` | 2 | ✅ 会建几何 |
| `Revolution` / `NegExtrusion` / `NegRevolution` / `NegPolyhedron` | 各 1 | ✅ 会建几何 |
| `RouteMember` | 85 | 记账，几何压算法线 P4 |
| `NonGraphic` | 14 | ⚠️ **要逐条复核**，见缺陷 5 |
| `Catalog` | 9 | 记账，硬依赖 G4 目录求值 |
| `NegUnimplemented` | 6 | ⚠️ 其中 3 个家族判错，见缺陷 2 |
| `List` | 5 | 合理（CWALL/CFLOOR/CMPF/FRMW/SBFR 靠 DFS 下钻等效） |
| `Unsupported` | 4 | 记账（CONE/DISH/SLCY/SNOU） |
| `ProfileData` | 3 | ⚠️ SPINE 存疑，见缺陷 6 |
| **`Unknown`** | **129** | ★ **无名目，本计划主攻** |

### 3.1 129 个欠账 noun 的确切名单（按家族，带 `*` = Core3D 现代 plug 已挂）

**IPFCOM 板墙楼板 / 建筑装配族（42）**
```
BPANEL* BPFITT BPOPEN CBSEG CLNCGR COMFIX CTFEAT CTRAY CTWALL* DOOR ELFITT*
ENDATU FIXTUR FPFITT* FURNIT HRGATE HRPANE HRPOST* HRTERM HVACFI* INFITT*
LADDER LDRCAG LDRRUN PLOPEN PLTFRM RAILSE RBRAN RLADDR* RLGATE RSEG RUNGSE
SLADDR* STRFLT* TREAD TREADS WINDOW WLFITT WLJOIN* WLOPEN WLPANE* WLPROF*
```

**IGMCOM 几何集（21）**
```
BOXI GMSE LCYL LINE LPYR LSNO SBOX SCON SCTO SCYL SDIS SDSH SEXT SLINE SLOO
SREV SRTO SSLC SSPH SVER TUBE
```

**IECOMP 设备图元（16）** —— 前 11 个是 A\* 族，见缺陷 3
```
ABOX ACONE ACTOR ACYLI ADISH APOLYH APYRA AREVO ARTOR ASLCY ASNOU
EQUCOM GENCUR GENPRI MNOZ SPMSPC
```

**ICABCO 电缆桥架（14）**
```
CABLE* CTBEND* CTCROS* CTJOIN CTMTRL CTREDU* CTRISE* CTSTRA* CTSUPP CTTEE*
CWAY CWBRAN* HATTA* RATTA
```

**INGCOM 负几何集（13）**
```
NBXI NGMS NLPY NLSN NSCO NSCT NSDS NSEX NSRE NSRT NSSL NSSP NTUB
```

**IFCOMP 结构件（10）**：`COFI CSCREE LCDE NODI NOLO PCOJ RELE RPLG SDLO SPLO`
**IG2COM G2 几何集（5）**：`GMSS SANN SPRO SPVE SREC`
**ICCOMP 目录件（4）**：`JOIN SCOM SFIT SPRF`
**IPCOMP 管件（3）**：`INSU TCOM TRAC`
**INCOMP 负图元（1）**：`NSLC` ← 见缺陷 2

### 3.2 一条独立交叉验证（好消息，说明路由那条线是稳的）

`route-nouns.json` 的 104 个路由成员是按**属性结构**（同时有 `ARRI`+`LEAV`）从 E3D 3.1
筛出来的，与本轮的字典家族位是**两条完全独立的证据**。对上号的结果：

- 55 个带 `IPCOMP`（管件）+ 30 个带 `IHCOMP`（吊架）= **85 个落在级联内**；
- 7 个（`PEXSP PGBOX PIACT PIBLK PIGEN PWCHA PWMAN`）2.10 字典里没有 = 3.1 新增 noun；
- 5 个路由容器 `BRAN/HANG/LUG/SUPC/TRUNNI` **一个家族位都不带** —— 与
  「容器自己不出几何、几何在成员上」的设计判断**完全一致**。

两条独立判据在 85 个 noun 上重合，路由族的分类可以按已定论处理，本计划不再动它。

---

## 四、必须改的六个具体缺陷

按「改动成本 ÷ 后果严重度」排序，前四个都是几行代码的事。

### 缺陷 1 — `POGON` 这个 noun 根本不存在（多面体的账是错的）

`category.rs` 的 `ProfileData` 名单里写着 `POGON`。**两份字典里都查无此 noun。**

真正的面 noun 有两个，字典与既有计划引用的码逐位相符：

| 用途 | 正确 noun | 码 | 既有计划 §3.7.4 引的码 |
|---|---|---|---|
| 顶点表 | `POLPTL` | 183671242 | 183671242 ✅ 对上 |
| **现代 POLYHE 的面** | **`POLFAC`** | **44236870** | 44236870 ✅ 对上，但代码里写成了 POGON |
| 旧式 POHE 的面 | `POGO` | 832210 | —（在 `sub_10343B80` 的码组里） |

**后果不是建不出几何，是账错了**，这更难查：`polyhedron.rs` 按**位置**走成员
（首成员 `POLPTL`，其余即面），几何照出；但 `pipeline.rs` 的 `push_members` 按
**noun 名**判要不要消费——`POLFAC` 落 `Unknown` → 不消费 → 入栈 → `visited++` 且
计进 `unknown_nouns`；它下面的 `POLOOP`/`LOOPTS` 因为属主类别是 `Unknown`
（`consumes_members()==false`）而全部计进 `orphans`。

于是一个正常建出来的多面体，会在报告里同时留下「未知 noun」和「孤儿剖面」两笔坏账。
`accounts_for` 还是平的（元素没蒸发，只是记错本），所以**这个错永远不会让门禁变红**。

**改法**：`POGON` → `POLFAC`，并补 `POGO`。加一条回归：每个 ProfileData 名单里的
noun 都必须能在 `output/noun_family_matrix.json` 里查到。

### 缺陷 2 — `NSLC` 整族漏掉，`NSCY`/`NSBO`/`NLCY` 认错了家族

字典证据（`primitive`/`geomset` 能力位 + 家族位）：

| noun | 能力位 | 家族 | 含义 | e3d-model 现状 |
|---|---|---|---|---|
| `NSLC` | `primitive` | **INCOMP** | 设计负图元，正体 `SLCY` | ★ **落 Unknown，整个漏掉** |
| `NSCY` | `geomset` | **INGCOM** | 几何**集**负体 | 误列进设计负图元的 `NegUnimplemented` |
| `NSBO` | `geomset` | **INGCOM** | 同上 | 同上 |
| `NLCY` | `geomset` | **INGCOM** | 同上 | 同上 |

12 个设计负图元（INCOMP）的逐条状态：

| noun | e3d-model | 正体（字典） | 正体状态 |
|---|---|---|---|
| NBOX NCYL NPYR NRTO NCTO | ✅ `NegPrimitive` 建 | BOX CYLI PYRA RTOR CTOR | ✅ 建 |
| NXTR / NREV / NPOLYH | ✅ 建 | EXTR / EXTR / POLYHE | ✅ 建 |
| NCON NDIS NSNO | `NegUnimplemented` 不建 | CONE DISH SNOU | `Unsupported` |
| **NSLC** | ★ **Unknown** | SLCY | `Unsupported` |

**这条同时修正既有计划 §2.3 的一句结论。** 那里写的是：

> 「e3d-model 写的 `NSCY` 在 core.dll 注册表里是 `NSLC`；`NSBO`/`NLCY` 在注册表里**根本不存在**」

前半句对，后半句**错**：`NSBO`/`NLCY` 是**存在的真 noun**，只是不在 Core3D 的
`primList_` 里，因为它们属于**另一个几何家族**（几何集 INGCOM，走
`CGTCT2 + sub_10714FC0`，不是设计图元路）。既有计划 P1 写的处置动作是
「`NSBO`/`NLCY` 若注册表确无则**删或降为 Unknown**」——**按本轮证据这个动作是错的**，
删掉它们等于把几何集负体这一族从账上抹掉。正确动作是给它们一个新类别，见缺陷 4。

### 缺陷 3 — `A*` 设备图元族 11 个整族漏，其中 5 个近乎零成本

`IECOMP` 家族里有一整套 `A*` 图元，与设计图元一一对应：

```
ABOX ACONE ACTOR ACYLI ADISH APOLYH APYRA AREVO ARTOR ASLCY ASNOU   ← 全部落 Unknown
AEXTR                                                               ← 已认领为 LoopExtrusion
```

**`AEXTR` 已经在 `LoopExtrusion` 名单里，它 11 个兄弟一个都没有。** 这是典型的
「认领了一个、以为认领了一族」——而漏掉的表现只是报告里多几行计数，不报错。

其中 **`ABOX`/`ACYLI`/`APYRA`/`ARTOR`/`ACTOR` 与已实现的 `BOX`/`CYLI`/`PYRA`/`RTOR`/`CTOR`
同形**，接进已有的 `Primitive(Prim::*)` 即可，成本≈改 5 行分类表 + 5 条断言。
**但前提是先坐实它们的属性读法与设计图元一致**（`A*` 大概率是「附加/关联图元」，
尺寸属性名可能不同）——不许凭名字像就直接接线，见 N1 的验收条件。

### 缺陷 4 — 几何集家族（42 个）在 e3d-model 里连类别都不存在

`IGMCOM`(21) + `INGCOM`(16) + `IG2COM`(5) 是 core.dll 里**独立的一条几何路**
（`CGTCT2` + `sub_10714FC0`，与设计图元的 `sub_10343B80` 并列）：

```
正体：BOXI GMSE LCYL LINE LPYR LSNO SBOX SCON SCTO SCYL SDIS SDSH SEXT SLINE
      SLOO SREV SRTO SSLC SSPH SVER TUBE | GMSS SANN SPRO SPVE SREC
负体：NBXI NGMS NLCY NLPY NLSN NSBO NSCO NSCT NSCY NSDS NSEX NSRE NSRT NSSL NSSP NTUB
```

42 个里 39 个落 `Unknown`，剩下 3 个（NSCY/NSBO/NLCY）是缺陷 2 里认错家族的那三个。

**`Category` 枚举里没有任何一项对应这条路。** 这不是「实现没跟上」，是**建模概念缺失**——
分类表的设计前提是「noun 要么是设计图元、要么是环拉伸、要么是目录件、要么是路由件」，
几何集这一路不在这四类里的任何一类。

**本计划只要求给它一个名目**（`Category::GeometrySet` / `NegGeometrySet`，记账不建），
**不要求本期建几何**。理由：ams1112/ams8000 语料里一件都没有，先建就是无验收的投机；
但没有名目，它们会一直混在 `unknown_nouns` 里，把真正的欠账淹掉。

### 缺陷 5 — `NOZZ`/`ELCONN` 判 `NonGraphic`，与 GTGEOM 的硬编码直接冲突

`GTGEOM` 的 `IPCOMP|IHCOMP` 分支**硬编码**了三个 noun 码走复合几何：

```
1062572 = NOZZ      209154074 = ELCONN      195113669 = EQUCOM
```

而 `category.rs` 把 `NOZZ`、`ELCONN` 都放进 `NonGraphic`（「无三维表示」），
`EQUCOM` 落 `Unknown`。`NonGraphic` 在 `pipeline.rs` 里与 `List` 共用一个空分支
（`Category::List | Category::NonGraphic => {}`）：元素进了 `visited` 总数，
但**没有任何按 noun 的分账**——报告里看不出哪些 noun 是以「无三维表示」为由被放过的。

管嘴（NOZZ）在设备模型里是有实体几何的，判成「无三维表示」很可能是整族漏建，
而且是**报告里完全看不见**的那种漏。

级联内共 **14 个** noun 被判 `NonGraphic`，全部要逐条复核：
```
ELCONN[IECOMP]  NOZZ[IECOMP]   JLDATU[IPFCOM] PLDATU[IPFCOM] PNOD[IFCOMP]
SNOD[IFCOMP]    PJOI[IFCOMP]   SJOI[IFCOMP]   SCOJ[IFCOMP]   SELJ[IFCOMP]
SUBJ[IFCOMP]    PALJ[IFCOMP]   SEVE[IFCOMP]   RNODE[ICABCO]
```

> 注：带家族位**不等于**一定出几何——`create_geometry` 进去后可能因为没有几何成员
> 而空手出来。所以这条的动作是**复核 + 给依据**，不是无脑改成建几何。
> 但「判 NonGraphic」这个结论现在**没有任何证据支撑**，而反方向有 core.dll 的硬编码。

### 缺陷 6 — `SPINE` 带 `IPFCOM` 家族位，却被当剖面数据，且实跑里正在漏

`SPINE` 在字典里 `primitive=true` + `IPFCOM` 家族位 → core.dll 会送它进复合几何路。
e3d-model 判 `ProfileData`（属主消费、不下钻）。

而 `out/ams1112/report.json` 里 **`orphans: {"CURVE": 14, "SPINE": 14}`** ——
这 14 个 SPINE **没有属主消费它们**，正以孤儿身份挂在账上。既然没人消费，
判 ProfileData 就等于让它们什么都不出。`CURVE` 同样 14 个孤儿（`primitive=true`，
但无家族位，性质待定）。

两个数字一样是 14，指向同一批元素（SPINE 下挂 CURVE）。这是一条**实跑已经在报警、
但报警级别被设成 orphan（低）**的线索。

---

## 五、完成判据

- [x] **判据可机械复算**：`tests/noun_coverage.rs` 6 条回归把 `classify()` 对全表跑一遍，
      与承诺的覆盖矩阵比对，不符即红。**影子表已删**（§2.3 的失效点消除）。
- [x] **级联内 `Unknown` 清零**（129 → 78 → **0**）：每个 noun 都进了实义类别，
      并由 `dump_categories` 的 `evidence` 字段带一行依据。
- [x] `unknown_nouns` 语义收窄成「字典里也没有的 noun」：ams1112 与 ams8000 两个语料
      实跑的 `unknown_nouns` 均为空，出现即真报警。
- [x] 六个缺陷逐条关闭（1/2/3/4 记账 → N1；5/6 → N2），各有回归测试钉住：
      `the_n1_defects_are_closed`、`the_n2_class28_fix_is_closed_and_spine_is_still_evidence_pending`、
      `every_noun_named_in_the_classifier_actually_exists_in_the_dictionary`（钉缺陷 1 那类错）。
- [x] 分账可按专业线切：`cascadeFamilies`（11 族 327）与 `nonCascadeFamilies`（8 族，
      含船体 `IHLCOM` 96）在矩阵里分开列，终态表 §二分两张表，不混。
- [x] `docs/` 下的 NOUN 覆盖矩阵终态表：**`vendor/e3d-model/docs/noun-coverage-matrix.md`**，
      19 个家族逐个写终态，无空格。唯一的「判不了」是 `IHVCOM`，已显式标注原因
      （字段 591821 两版字典都不存在）而不是含混地写成「无几何」。

---

## 六、阶段

### N0 — 把审计判据从「Python 影子表」换成「Rust 侧真表」

状态：**done（2026-08-31，会话 7KG8）**。这是整个计划的地基。

原先的 `classify()` 副本是手抄的（`.scratch/noun_coverage_audit.py`），
`category.rs` 一改它就悄悄过期——用一张会过期的表去审「有没有漏」，是自相矛盾的。

- [x] `vendor/e3d-model` 加 `src/bin/dump_categories.rs`：读家族矩阵，对每个 noun 调真
      `classify()`，输出 `out/noun-categories.json`（逐 noun 判词 + 汇总数）。
- [x] 审计脚本换成 `scripts/noun_coverage_report.py`，只消费该产物、**不含任何
      noun→类别对照表**；`.scratch/noun_coverage_audit.py`（影子表）与依赖它的
      `.scratch/coverage_crosscheck.py` 已删。
- [x] `noun_family_matrix.json` 收进 `vendor/e3d-model/data/noun-family-matrix.json`
      （与 `route-nouns.json` 同级），头部带来源、字典版本、推导路径、级联家族名单。
- [x] `icom_field_ids.py` + `noun_family_probe.py` 挪进 `vendor/e3d-model/scripts/`，
      脚本头写清重跑步骤（换 E3D 版本时要重跑，见 N4）。
- [x] `tests/noun_coverage.rs` 5 条回归钉住底账，`cargo test --test noun_coverage` 全绿。

验收（已达成）：影子表删除后审计仍可一键复算——
`cargo run --bin dump_categories -- data/noun-family-matrix.json out/noun-categories.json`
接 `python scripts/noun_coverage_report.py`；判据只有 `classify()` 一处，改一个臂
审计结果当场变，且回归测试会当场红。

**N0 落下来的五条护栏**（后续阶段动 `category.rs` 时会先撞上它们）：

| 测试 | 钉住什么 | 改动时的预期反应 |
|---|---|---|
| `cascade_coverage_matches_the_recorded_baseline` | 277 分母 / 22 建 / 126 记账 / 129 欠账 | N1、N3 每关一批就要改这四个数，等于强制更新覆盖率口径 |
| `the_unknown_backlog_is_still_spread_across_the_same_families` | 129 个欠账在 10 个家族里的分布 | 防「只挪账不减账」——换个类别名但没真减少欠账会被这条挡住 |
| `every_noun_named_in_the_classifier_actually_exists_in_the_dictionary` | 扫 `category.rs` 里所有大写字面量，逐个查字典 | 现在断言恰好等于 `["POGON"]`；缺陷 1 修完这条会红，逼着把 `KNOWN_BOGUS` 清空 |
| `negative_primitives_and_their_positive_twins_agree` | 正负体建/不建一致 | 只补负体不补正体（或反之）会红 |
| `known_classification_gaps_are_still_open` | 缺陷 2、3 的现状 | **特征化测试**：修好缺陷它就红，逼着改测试并确认修复真落地 |

第三条值得单说：它不是靠一张 noun 名单，而是正则扫 `src/category.rs` 的源码文本取
所有 `"[A-Z0-9]{2,8}"` 字面量再查字典。也就是说以后往分类表里写任何一个新 noun，
只要名字打错，这条测试立刻抓住——缺陷 1 那类「名字看着像、其实字典里没有」的错
从此进不来。

**N1 实测反应**（五条全按预期动了，无一条是「改测试让它过」）：前两条各红一次，
按新四数与新家族分布改；第三条在缺陷 1 修完后红，`KNOWN_BOGUS` 已清空；第四条
因 `NSLC` 补齐而由「正负不对称」转绿；第五条（特征化）如设计地红了，已拆成
`the_n1_defects_are_closed`（断言修复真落地）与 `the_n2_defects_are_still_open`
（继续钉住缺陷 5、6 的现状，留给 N2 撞）。

### N1 — 关掉四个低成本缺陷（1/2/3 + 缺陷 4 的记账部分）

状态：**done（2026-08-31，会话 11fd1e5f）**。依赖 N0。

- [x] 缺陷 1：`POGON` → `POLFAC`(44236870)，补 `POGO`(832210，旧式 `POHE` 的面)。
      两个真名都从家族矩阵查得到，`KNOWN_BOGUS` 随之清空。
- [x] 缺陷 2：补 `NSLC`（→ `NegUnimplemented`，与未实现的正体 `SLCY` 同步）；
      `NSCY`/`NSBO`/`NLCY` 移出设计负图元，改归 `NegGeometrySet`。
- [x] 缺陷 3：取证后接线。取证走的不是原计划的「读 ams 库元素」（级联里
      `A*` 元素一件都没有，读不到），而是比对 `all_attr_info.json` 的属性表：
      `ABOX/ACYLI/APYRA/ARTOR/ACTOR` 的量纲属性与 `BOX/CYLI/PYRA/RTOR/CTOR`
      **逐个同名同义**，差的只有 `LEVE/OBST/POSI` 这类非几何簿记属性，故接进
      `Primitive(Prim::*)`；脚本 `scripts/a_family_attr_probe.py` 可一键复算。
      `ASLCY/ACONE/ADISH/ASNOU` 归 `Unsupported`（正体本就没实现，等正体一起接），
      `APOLYH/AREVO` 也归 `Unsupported`——它们的几何来自成员结构（B-rep 顶点面表 /
      `PLOO` 剖面环），属性表比对说不了成员的事，不按「看着像」接线。
- [x] 缺陷 4 的记账部分：`Category` 新增 `GeometrySet` / `NegGeometrySet`，
      42 个 noun 全部认领；`Report` 加 `geometry_sets` / `geometry_set_negatives`
      两本账；`elmodl.rs` 的 `negative_world_solid` 补对应臂（否则撞 `unreachable!`）。
      **本阶段不建几何。**

验收（已达成）：

- 级联内 `Unknown` **129 → 78**，与预期的 51 笔（`NSLC` 1 + `A*` 11 + 几何集 39）
  逐笔对上；四数变成 277 分母 / **27** 建 / **172** 记账 / **78** 欠账。
- `NegUnimplemented` 从 6 收敛到 4（`NCON`/`NDIS`/`NSNO`/`NSLC`，全是设计负图元）。
- `cargo test --lib --test noun_coverage` 全绿，`cargo build --all-targets` 通过。
- ams1112 重跑（`out/ams1112-n1`）：`visited=6115 generated=4476 unknown=0
  orphans=28`，账平（`visited + consumed = 30940 =` 索引数）。两本账逐条能解释——
  `unknown_nouns` 空；`orphans` 只剩 `CURVE` 14 + `SPINE` 14，正是缺陷 6，归 N2 取证。

**顺手补的一处护栏**：`builds` 判据原先在 `dump_categories.rs` 与 `noun_coverage.rs`
里各写一份 `matches!`，新增类别时漏改一处就会出现「审计说建、测试说不建」的静默分歧。
本阶段收敛成 `Category::builds_geometry()` 一处，两边都调它。

### N2 — 复核 14 个 `NonGraphic` 与 3 个 `ProfileData`（缺陷 5、6）

状态：**done（2026-08-31）**。依赖 N0。**这一阶段是取证，不是实现。**

> 换 3.1 字典后 `NonGraphic` 从 14 收到 **12** 个、`ProfileData` 仍 3 个（`PAVE PLOO SPINE`）。

- [x] 活桥反编译坐实 class 28 这条路：`GTGEOM(0x10341d2e) → sub_10714FC0 → create_geometry`，
      `sub_107189A0` 把 `NOZZ`/`ELCONN`/`EQUCOM` 映射为 **28**（`IPCOMP`/`IHCOMP` 为 29/30），
      `sub_107210A0` 按成员顺序创建并串接 member builder。三个 noun 因此从 `NonGraphic`
      改判 `Composite(CoreClass28)`——**缺陷 5 关闭**。
- [x] `NonGraphic` 不再与 `List` 共用空分支：`pipeline.rs` 立了 `proven_non_graphic` 一本账，
      连同 `evidence_pending` / `composites_built` / `noun_evidence` 四本一起进报告，
      「判它无几何」这个结论现在**能被实跑证伪**了。
- [x] `SPINE`/`CURVE`：ams1112 那 28 个孤儿已按属主语义收编——14 组 `WALL → SPINE → CURVE`
      是目录墙的路径数据，由属主消费。实跑 `out/ams1112-n3` 的 `orphans` 从
      `{CURVE:14, SPINE:14}` 变成**空**，`consumed` 多出 `SPINE: 56`，`visited` 相应减 56，
      `visited + consumed = 30940` 仍等于索引全集——**缺陷 6 的实跑症状消失**。
- [x] 结论逐条写回覆盖矩阵并带出处：`docs/noun-coverage-matrix.md` §五 列出 7 类 `evidence`
      文案及条数，class 28 那 3 条直接写函数地址。

验收（已达成）：12 + 3 个 noun 每个都有一行带出处的终态；`NonGraphic` 在报告里可计数
（ams1112 记到 `JLDATU 252 / PLDATU 252 / SNOD 2 / PJOI 2 / MNUM 1 / PNOD 1 / SJOI 1`，
ams8000 记到 `SCOJ 389 / SNOD 389 / SUBJ 428 / PJOI 248 / PNOD 248 / TEXT 24 / MNUM 1`）。

**半边证据已于 2026-08-31 19:3x 补齐 —— class 28 三件全部落地**：

原缺口是 class 28 的实跑证据只覆盖 `ELCONN`（ams8000 有 34 件，全部无成员，
因此停在 `EvidencePending`、没有冒充已建）；`NOZZ` 与 `EQUCOM` 在 `ams1112`/`ams8000`
两个语料里一件都没有，因为这两个库都出自 `AvevaMarineSample`，本就没有设备库。

换到工厂样例 `AvevaPlantSample`（`aps000` 下 240 个库），用 `examples/tree_census`
逐库清点找靶（脚本 `gen-model/.scratch/t88w_find_nozz.ps1`，234 个候选全扫 63 秒，
命中 18 个含设备族的库），选定两个：

| 库 | 元素 | 总账 | class 28 落账 |
|---|---:|---|---|
| `aps250164_0001` | 4 071 | `visited=1048 consumed=3023 generated=905 failed=0 orphans=0`，账平 | `NOZZ 50`、`ELCONN 8` |
| `aps250209_0001` | 20 994 | `visited=7361 consumed=13633 generated=1801 failed=0`，账平 | `EQUCOM 438` |

三个 noun 全部落在 `evidence_pending` 且带 class 28 出处串，**没有一个进 `unknown_nouns`**；
`aps250209` 的 DFS 可达数 `索引 20993 / DFS 20993`，一个没漏。

**顺带坐实一条更强的结论**：`tree_census` 父子表（已确认未截断）显示
`NOZZ`/`EQUCOM`/`ELCONN` **一次都没有作为父出现** —— 三者在真库里全是叶子。
所以 `sub_107210A0` 那条成员聚合链在手上所有语料（共 496 件）里都无物可聚。
把三者停在 `EvidencePending` 而不硬做聚合 builder，**没有丢任何几何**。

**新语料同时暴露两笔新账，都不影响四数基线**：

1. 14 个未知 noun（`aps250164` 3 个共 10 件、`aps250209` 11 个共 1438 件）。
   逐个查家族位（`gen-model/.scratch/t88w_unknown_probe.py`）：13 个不带任何家族位，
   `DPCA` 带级联外的 `IPTCOM`。级联内判 `Unknown` 的 noun 仍是 0 个。
   ⚠ 但「级联外」不等于「没几何」——`HANDRA`（扶手）、`CONVEY`（输送机）看着就有形，
   只是不走 GTGEOM。需要单独定性，别拿「级联外」当「已收口」。
   **2026-08-31 20:5x 定性已完成**（会话 RJYQ，详见 `docs/noun-coverage-matrix.md` §九）：
   真欠账 1 个（`HANDRA`）、当前处理有缺陷 1 个（`TMPL`）、几何载体 2 个
   （`RPATH`/`POINTR`）、组织容器 1 个（`CONVEY`，应同 `HVAC`/`PIPE` 归 `List`）、
   非图形数据 9 个。硬出处：`core.dll` `IASLCO`（扶手/平台总成族的写死名单，
   `HANDRA` 与 `HRPOST`/`HRPANE`/`HRGATE`/`HRTERM`/`PLTFRM`/`STRFLT` 同族）、
   `LPRMTV`（「属主是 `TMPL` 即算图元」，且显式排除 `DDSE`/`DPSE`）。
   **同时查出这 14 个不是全集**：换 `aps250110_0001` 一个库就多出 9 个
   （`POSTSE HRKPSE HRPNSE HRFEAT RAIL KICKPL RLCAGE CAGSEG HOOPSE LDRSTR STRSTR
   DPSP TMRREL`），其中 `RAIL`/`KICKPL` 是扶手上的实体件。要全集得按库遍历。
2. 17 个孤儿负体（`NCON 3 / NCYL 12 / NPYR 2`），根因对上父子表正好是
   `CONE → NCYL 12 / NCON 3 / NPYR 2`：属主 `CONE` 判 `Unsupported`（本期没实现），
   负体自然没人消费。不是分类缺陷，是孤儿账正常发挥作用——它指出将来实现 `CONE`
   时这 17 个负体必须一起接进去，漏了的表现是该有洞的地方没洞。

### N3 — 剩下 78 个的记账（IPFCOM 42 / ICABCO 14 / IFCOMP 10 / IECOMP 5 / ICCOMP 4 / IPCOMP 3）

状态：**done（2026-08-31）**。依赖 N1。

这两族里有 13 个（带 `*`）**已经有 Core3D 现代 CSG plug**，属现代路，与既有计划
§2.2 的 builder 表能对上号；其余走旧路。

- [x] 认领方式最终没有按「现代路 / 旧路」分两栏，而是按**业务家族**开
      `Composite(CompositeFamily)`：`Structural` 59、`CableTray` 19、`BuildingAssembly` 17、
      `PipeComponent` 12、`Catalogue` 6、`Equipment` 4、`CoreClass28` 3。
      理由：现代路与旧路的差别是「builder 在哪」，而记账要回答的是「走哪条 builder 路」，
      后者才是 N4 换字典、算法线 P3+ 接实现时真正会用到的切分。
- [x] 电缆桥架族判断已下：`ICABCO` 21 个整族归 `Composite(CableTray)`，**不复用**
      `RouteContainer`/`RouteMember` 的串接逻辑（判断而已，本期不实现）。
- [x] 尾部零散的也全部认领，级联内 `Unknown` **清零**。
- [x] 覆盖矩阵 19 个家族逐个写终态：`vendor/e3d-model/docs/noun-coverage-matrix.md`。

验收（已达成）：级联内 `Unknown` **0**（换 3.1 字典后分母 327，`dump_categories` 打印
「命中 GTGEOM 级联 327：会建 27 / 记账不建 300 / 分类表欠账 0」）；两个语料实跑的
`unknown_nouns` 也都是空——ams8000 原本的 `{HVAC:2, TEXT:24}` 已归位（`TEXT` 判
`NonGraphic`，`HVAC` 判 `List`，后者是纯组织容器、按设计不单独记账，有测试钉住）。

**未做的一项**：与既有计划 §2.2 的 19 个 builder 表逐条对账（两张表互相指认）没做。
它不影响 `Unknown` 清零，但算法线 P3+ 接实现前应当补上。

### N4 — 版本口径收口

状态：**done（2026-08-31，残留已于 18:5x 由会话 T88W 补齐）**。可与 N1–N3 并行。

- [x] 3.1 的完整 `attlib.dat` 找到了：`E:\reverse\e3d\shadow_e3d31_aps_all\attlib.dat`
      （5 840 896 bytes，不是 `Data\DFLTS3.1` 下那个 9.7KB 的缺省桩）。矩阵已按它重算，
      noun 从 1384 涨到 **1931**，「548 个 noun 没有家族位数据」这条**结掉**。
- [x] 重跑 §2.2 后级联分母 **277 → 327**。矩阵头部带 `dictionaryVersion` 版本戳，
      换版本重跑就是一条命令（`python scripts/noun_family_probe.py <attlib> --version <名>`）。
      矩阵**可复现**：T88W 用同一条命令重算到临时路径，与落盘的
      `data/noun-family-matrix.json` SHA-256 逐字节一致
      （`13851F8024E6D4F4339F38FD453F2FB50E45999D07255D2E5A3E839048548EEF`）。
- [x] `IHVCOM` 已按要求**显式标注「本轮判不了」而不是「无几何」**，写在
      `docs/noun-coverage-matrix.md` §二。实测结论比原先更强：字段 `591821`
      在 **3.1 字典里同样不存在**（探针照常打印 `⚠ …本轮判不了：[591821]`），
      所以「命中 0」的含义是读不到这一位。`HVAC` noun 自身判 `Category::List`
      （`hard base` 是 `PIPE`，纯组织容器），那是另一条独立证据。
- [x] 覆盖矩阵带字典版本戳并落盘：`vendor/e3d-model/docs/noun-coverage-matrix.md`。
- [x] **2.10 ↔ 3.1 家族位 diff 明细表**：`docs/noun-coverage-matrix.md` §七。
      2.10 那版矩阵用同一个探针重算（198 332 bytes / 277 个级联，与 N0 时的原件同尺寸），
      再与 3.1 逐 noun 对照，差异逐条过完。

验收（已达成）：矩阵可标注版本、一条命令复算 ✅；3.1 与 2.10 的差异有明细表 ✅。

**diff 的四条结论**：

1. **分母 +50 拆得干净**：49 个 3.1 新增 noun 直接落级联 + `TATTA` 一个老 noun 新加
   `ICABCO` 位 − **0 个退出** = +50。**没有任何 noun 在 3.1 里掉出级联**，
   这条反向变化的排除比增量名单本身更重要——它说明换版本只加不减，旧结论不会被推翻。
2. **50 个全部已分类、无一落 `Unknown`**，终态清一色 `EvidencePending`
   （`Structural` 17 / `PipeComponent` 9 / `BuildingAssembly` 7 / `RouteMember` 7 /
   `CableTray` 5 / `Catalogue` 2 / `GeometrySet` 2 / `NegUnimplemented` 1）。
3. **HVAC 的几何走 `IPCOMP`，与读不到的 `IHVCOM` 无关**：新进 `IPCOMP` 的 16 个里有 9 个
   `HV*`（`HVBRCO HVFLAN HVHACC HVIDAM HVSADD HVSKIR HVSPLR HVSTIF HVTPPO`），
   正是 Core3D 现代 plug 已挂的那批。所以「`IHVCOM` 判不了」不影响 HVAC 部件进级联。
   另外 7 个（`PEXSP PGBOX PIACT PIBLK PIGEN PWCHA PWMAN`）正是 §3.2 里
   「`route-nouns.json` 有、2.10 没有」的那 7 个，换 3.1 后归位 `IPCOMP`，该残留闭合。
4. **`positive_equivalent` 两版零变化**，正负配对不受换版本影响。

**diff 顺带查出一处记账不自洽 —— 已改判闭合（2026-08-31 19:0x）**：

`CTRAY` 在 3.1 里多了 `ICABCO` 位，于是同时带 `IPFCOM`+`ICABCO`，是全字典唯一一个同时带
两个复合家族位的 noun。`classify()` 的家族兜底是一条有序 `if` 链、`IPFCOM` 原先排在
`ICABCO` 前面，于是桥架主干 `CTRAY` 落进 `Composite(Structural)`，而它的弯头、直段
`CTSTRA`/`CTBEND`/`CWAY`/`CWBRAN`/`TATTA`/`HATTA`/`RATTA` 全在 `Composite(CableTray)`。
本计划 §3.1 也把 `CTRAY` 归在电缆桥架族。这个矛盾在 2.10 下不存在（那时它只有 `IPFCOM`），
**是换 3.1 之后才出现的，属于换版本引入的新账**。

改法与波及面：

- 把 `ICABCO` 一支提到 `IPFCOM` 之前。理由是电缆桥架是一条自带容器（`CWAY`/`CWBRAN`）
  与成员的独立专业线，带这个位就该按桥架记账；**不是「谁的族小谁优先」**——`IFCOMP`/
  `IECOMP` 仍留在 `IPFCOM` 之后，同时带 `IFCOMP`+`IPFCOM` 的 `STWELD` 照旧归 `Structural`。
- 波及面经字典实算确认**只有 `CTRAY` 一个 noun**（全字典带两个级联家族位的仅
  `CTRAY`、`STWELD` 两个，后者不含 `ICABCO`）。复算：`gen-model/.scratch/t88w_icabco.py`。
- **不改变任何几何结论**：core.dll 里 `IPFCOM` 与 `ICABCO` 两个谓词都进同一个
  `sub_10714FC0`（见 §2.1 GTGEOM 去向表），走哪个位进去都是同一条复合几何路。
- 四数 **1931 / 327 / 27 / 300 / 0 一位没动**；位移只在类别分布：
  `Composite(Structural)` 59→58、`Composite(CableTray)` 19→20。
- 回归：新增 `category::tests::the_cable_tray_family_stays_together`，正面钉桥架
  五件（`CTRAY CTSTRA CTBEND CTREDU CTTEE`）整族在一起，反面钉 `STWELD` 仍归结构。
- 基线同步更新：`docs/noun-coverage-matrix.md` §二（`IPFCOM`/`ICABCO` 两行）、§四
  （两族计数）、§7.3（改判记录）。§7.2 的 +50 分布不受影响（`CTRAY` 本就在 2.10 级联内，
  不属于新进级联的那 50 个），已复算确认。
- 全量测试：104 单测 + 6 覆盖测试 + 6 `rvm_compare` + 1 `increment_real`，全绿。

---

## 七、风险、残留与要回填的既有文档

### 7.1 要回填进 `2026-08-31-core-aligned-model-generation` 的三处

状态：**已回填（2026-08-31，会话 7KG8，用户拍板「现在就回填」）**。三处修正连同
`positiveEquivalent` 配对、22 个 `I*COM` 谓词全表已写进那份文档的对应节，并在其
「批注处置记录」表追加了本轮条目。以下保留原始措辞备查：

1. **§2.3 结论修正**：「`NSBO`/`NLCY` 在注册表里根本不存在」→ 应为「它们存在，
   属几何集家族（INGCOM），不在 Core3D `primList_` 里」。连带 **P1 的处置动作
   「删或降为 Unknown」是错的**，会抹掉一整族。
2. **§6.4 结案**：NOUN→db1 码表不必另求，就是 `noun_flags.json` 的 `noun_hash`；
   既有计划里所有硬编码码已反查完（本计划 §2.4）。
3. **§2.4.2 残留结案**：「exact `I*COM` 谓词拿不到」——本轮 `core.dll.i64` 上桥后
   22 个谓词的地址与字段号全部解出（本计划 §2.1）。

### 7.2 本轮新开的查证项

- **`NREV → EXTR` vs `CSG_BasicREV(REVO/NREV)`**：字典的 `positiveEquivalent` 与
  Core3D 注册表配对不一致。两者可能不矛盾（「等价 noun」≠「共用 builder」），
  但查清前不许拿字典配对驱动回转体建法。
- **`A*` 族的属性名是否与设计图元同名同义**：N1 缺陷 3 的前置，不许凭名字接线。
- **带家族位 ≠ 一定出几何**：`create_geometry` 可能空手而归。凡是靠家族位推断
  「应该建」的结论，落地前都要有第二条证据（反编译 / RVM 实测）。

### 7.3 风险

- ~~**审计脚本自己会过期。**~~ **N0 已消除。** 判据现在只有 `classify()` 一处，
  影子表已删；`tests/noun_coverage.rs` 会在分类表变动而底账没跟着改时报红。
  残留的同类风险只剩一个：家族矩阵 `data/noun-family-matrix.json` 是快照文件，
  换 E3D 版本时必须重跑 `scripts/noun_family_probe.py`，否则分母陈旧（见 N4）。
- ~~**129 这个数字会随字典版本变。**~~ **已兑现**：换 3.1 后 noun 从 1384 涨到 1931，
  级联分母 277 → 327。这正是「不要把它当固定 KPI」的实证——**分母涨了 50 但会建仍是 27**。
- **给类别名 ≠ 覆盖。** N1/N3 大部分动作是「让欠账变成有名目的账」，
  这提高的是**可观测性**不是**几何完整度**。别在汇报里把两者混着说。
  **已在代码层立防线**：`Category::build_disposition()` 给出 `Built` /
  `EvidencePending` / `ProvenNonGraphic` / `OutOfCorpus` 四态，与「类别」正交，
  报告里 `composites_built` 与 `evidence_pending` 分开记——300 个记账**不会**被算进覆盖率，
  当前几何覆盖率是 **27/327 = 8.3%**。
- **缺陷 1 那类错不会让门禁变红。** `accounts_for` 是等式判据（元素没蒸发就平），
  记错本它逮不住。所以每个缺陷都要配自己的回归测试，不能指望总账兜底。
- ~~**`NonGraphic` 现在连计数都没有**，导致「判它无几何」这个结论在实跑里无法证伪。~~
  **N2 已消除**：`pipeline.rs` 立了 `proven_non_graphic` 独立一本账，两个语料实跑都记得出
  逐 noun 的件数。
- **并发写同一个 crate。** `vendor/e3d-model` 历史上被多个会话实时改写且不在 git 里，
  动手前先确认无人在写。

## Errors Encountered

| Error | Attempt | Resolution |
|---|---|---|
| `pseudocode` 表查询必须带 `WHERE func_ea=` 或 `ea=` | 沿用既有计划记录 | 已知，直接按地址查 |
| `I*COM` 在 Core3D.dll 里只有 `__imp_` thunk | 前几轮在 Core3D 上查 | 换 `idalib-41236`（core.dll.i64），真身在这里 |
| `gm_noun_caps_probe.py` 默认路径 `Everything3D3.1\attlib.dat` 不存在 | 直接跑默认 | 本机实际是 `D:\AVEVA\Everything3D2.10\attlib.dat`（3.9MB）；3.1 目录下的 9.7KB 是缺省桩 |
| `IHVCOM` 的字段 591821 在字典 `field_index` 里查无 | 按字段号读 | 2.10 无此字段；**换 3.1 后仍无**（T88W 复算实测）。HVAC 家族判不了是常态而非版本问题，终态表里显式标「判不了」，不许写成「无几何」 |
| 反查 noun 数字码 | 以为要从 DLL 静态读（既有计划结论） | `DB_Noun` 静态镜像确实全 `0xffffffff`，但码就在 `noun_flags.json` 的 `noun_hash` 里 |


## 2026-08-31 ida-bridge 实施进度（实测账本）

> 本节只记录已经运行并有产物的结果；“已分类”不计入“已建”。

- **字典基线**：已用 E3D 3.1 `attlib.dat`（5,840,896 bytes）重算 327 个级联 NOUN。19 家族口径下，动态分类已做到 `unknown_nouns=0`；其中 27 个进入已有 builder，300 个仍为 `EvidencePending`，不能算几何覆盖。
- **route member 分区**：2.10 为 55 IPCOMP + 30 IHCOMP + 7 个 2.10 缺失 + 12 无家族位；3.1 中这 7 个已进入 IPCOMP，因此为 62 IPCOMP + 30 IHCOMP + 12 无家族位。5 个 route container 仍独立记账。
- **IDA 类 28**：已确认 `GTGEOM -> sub_10714FC0 -> create_geometry`，以及 `sub_107189A0` 将 `NOZZ/ELCONN/EQUCOM` 映射为 28、IPCOMP/IHCOMP 映射为 29/30。`sub_107210A0` 会按成员顺序创建并串接 member builder。AMS8000 的 34 个 ELCONN 均无成员，故目前保持 `EvidencePending`，没有冒充已建。**2026-08-31 补充**：APS 工厂语料实跑后，`NOZZ 50`（aps250164）、`ELCONN 8`（aps250164）、`EQUCOM 438`（aps250209）共 496 件全部落 class 28 账且**同样全是叶子、零成员**，三个 noun 的实跑证据就此补齐。
- **AMS1112（sesno 722）**：索引 30,940；`visited=6059`、`consumed=24881`、`unknown=0`、`orphans=0`、`failed=0`。实际生成 4,613（PANE 4,275、GWALL 120、FLOOR 81、SBFI 137）。14 组 `WALL -> SPINE -> CURVE` 已按属主语义消费；目录待实现 637（其中短 DESP 的 SBFI 3 个已从错误账移入目录待实现账）。
- **AMS8000（sesno 264）**：索引 6,605；`visited=6088`、`consumed=517`、`unknown=0`、`orphans=0`、`failed=0`。3,172 个管身槽位中实际建成 562，零长度终态 2,590，缺 P 点 19，另有 target-not-found 8、unbuildable 1。逐元素缓存开/关结果完全一致（715 元素，规范 JSON SHA-256 `60f6ae4629259950f8c8ead91c47bcf1a05f1ed5c0726a4ef5cea0ffd2089094`）。
- **TLEN/PTCD**：PTCD 已按 Core `DORTXT` 的轴链 + PML 角表达式实现；`FTUB + RPRO TLEN` 仅在该 noun/key 组合下映射到实例 HEIG，依据旧 E3D/RVM 对拍中 `PXLE=TLEN=1500` 且实例 `HEIG=1500`。启用 TLEN 后缺 P 点由 1,545 降到 19，实建管身由 329 增到 562。
- **仍未过门**：AMS1112 的 637 个目录 builder、AMS8000 的 550 个目录/路由构件实体、8 个跨会话目录引用、RVM facet 的 only-baseline/only-generated 与体积/连通分量门，以及真实增量回归 721→722 少删除 42 个几何。上述项目仍是待实现/待对拍，不计入完成。

## 2026-08-31 18:2x 回填记录（会话 T88W）

本节记录**谁在什么时候把文档追平到代码**，以及回填所依据的实测证据。

回填前的状态差：代码在 17:28–18:00 之间由并发会话推完了 N2/N3/N4 的主体，
但文档停在 17:11，三个阶段仍写着 `proposed`——**差三个阶段，会误导后续接手人**。

本轮改动的节：计划头部状态行与「权威」第 1 条（字典换 3.1）、§一 四数快照表、
§三 快照声明、§五 完成判据（7 条全部勾上）、N2 / N3 / N4 三个阶段的状态与验收、
§7.3 风险（三条已消除的划掉并写明消除方式）、Errors Encountered 的 `IHVCOM` 行。

回填所依据的实测（本会话跑的，命令与产物都可复算）：

| 证据 | 结果 |
|---|---|
| `cargo test --test noun_coverage` | 6 passed / 0 failed |
| `cargo build --all-targets` | 通过 |
| `dump_categories` 复算 | 3.1 字典 1931 noun；级联 327 = 会建 27 + 记账 300 + 欠账 0 |
| 家族矩阵可复现性 | 用同一条探针命令重算到临时路径，与落盘文件 SHA-256 逐字节一致 |
| ams1112 实跑 `out/ams1112-n3` | `visited=6059 consumed=24881 generated=4613`，账平；`orphans` 与 `unknown_nouns` 均空；既有 builder（FLOOR 81 / GWALL 120 / PANE 4275）零回归 |
| ams8000 实跑 `out/ams8000-n3` | `visited=6088 generated=153`，与改动前逐项相同；`unknown_nouns` 由 `{HVAC:2,TEXT:24}` 清空；`ELCONN` 34 件进 `evidence_pending` |
| `IHVCOM` 字段 591821 | 3.1 字典里同样不存在，探针照常告警 —— 判不了，不是无几何 |

新增产物：`vendor/e3d-model/docs/noun-coverage-matrix.md`（完成判据最后一条）。
过程记录：`gen-model/上下文/会话-2026-08-31-接力ZKNS-T88W-noun覆盖收口.md`。

**第二轮（18:4x–18:5x）**：补上 N4 的 2.10 ↔ 3.1 家族位 diff 明细表，写进
`docs/noun-coverage-matrix.md` §七，N4 残留就此闭合。复算脚本
`gen-model/.scratch/t88w_matrix_diff.py`，两版矩阵都可精确复现。
diff 顺带查出 `CTRAY` 的记账不自洽（换 3.1 引入，见 N4 段末）。

第二轮结束时本会话**未改动任何 crate 源码**（当时用户拍板「只做不写 crate 源码的活」，
因为并发会话仍在写 `category.rs` / `pipeline.rs`）。

**第三轮（19:0x）**：用户拍板「改 CTRAY 判词并更新基线」，授权就此放开。改动
`src/category.rs` 一个文件（兜底顺序 + 一条新回归测试），并同步 `docs/noun-coverage-matrix.md`
与本计划的基线数字。详见 N4 段末。改前已确认 `category.rs` 自 17:58 未被并发会话再动过。

**第四轮（19:3x）**：用户拍板「找带 EQUI/NOZZ 的语料补 class 28 实跑证据」。换到
`AvevaPlantSample` 找靶实跑，N2 的半边证据闭合，详见 N2 段末；新写 `docs/noun-coverage-matrix.md`
§八记录靶库、叶子结论、14 个级联外未知 noun、17 个 `CONE` 孤儿负体。
本轮**未改 crate 源码**，只是实跑 + 读报告 + 写文档。

**第五轮（20:3x–21:0x，会话 T88W → RJYQ 交接后）**：用户拍板「给 `HANDRA`/`CONVEY`
那 14 个级联外 noun 单独定性」。三路取证（AVEVA 官方元素类型说明 + `core.dll`/`Core3D.dll`
活体反编译 + 真库父子结构与实跑账），14 个逐个带出处结案，写进
`docs/noun-coverage-matrix.md` **§九**。

方法上有一条值得记的教训：第一轮立即数搜索搜了 `core.dll`，**对照组
`PANE`/`CTRAY`/`ELCONN`/`EQUCOM` 全部零命中**——因为几何分派逻辑整个在
`Core3D.dll`，`core.dll` 只装 `nounlib` 谓词本体。改搜后对照组 9 个全中，
「零命中」才第一次能当证据用。**这类判定必须带一组已知答案的对照。**

本轮查出两笔新账，都写进抬头残留：`TMPL` 的 1559 件负体成孤儿（孔洞静默丢失）、
`HANDRA` 一族是真欠账。另新增实跑产物 `out/aps250110-handrail/`。
本轮**未改 crate 源码**。
