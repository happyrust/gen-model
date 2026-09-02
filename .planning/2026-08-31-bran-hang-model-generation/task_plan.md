# Plannator 开发计划:e3d-model — BRAN/HANG 管路与吊架的模型生成算法

> 计划 ID:`2026-08-31-bran-hang-model-generation`
> 创建:2026-08-31 · 第二轮更新:2026-08-31(接力 M556 → 本会话)
> 状态:**draft**(待 plannotator 门禁)
> 上下文:`上下文/会话-2026-08-31-恢复c03e9977-管道库ams8000探针.md`(实时更新;
> 第二轮记在文末「第三段任务」一节)
> 同目录配套:`algorithm-bran-hang.md`(算法全文)、`route-nouns.json`(权威 noun 清单)
>
> **第二轮变更摘要(先看这个):**
> - **§6.1 第 1 条(管身两端与长度公式)已解出 → P2 解除封锁。** 算法全文见同目录
>   `algorithm-bran-hang.md`;一句话:长度 = 两 P 点直线距离,位置 = 中点,朝向 = `ORIMAT`。
> - **§6.1 第 2 条(管件族权威清单)已有出处** → 同目录 `route-nouns.json`,
>   路由容器 5 个 / 路由成员 104 个,与 core.dll 硬编码交叉验证通过。
> - **§6.1 第 3 条(`IPCOMP`/`IHCOMP`)已坐实**,且**推断有误需改**:`IHCOMP` 是吊架不是 HVAC。
> - **§6.2 第 6 条(`addNegatives` 白名单)结案**:没有白名单。
> - **§2.4「op=5 = 并集」的推断被推翻**,改为未定 + 给出验证方案(见 `algorithm-bran-hang.md` §5.4)。
>
> 拍板前提:
> - 2026-08-31 08:1x 用户:「测管道模型生成」→ 探针跑完 `ams8000_0001`(见 §3.1):
>   账目平衡、153 件直读几何全成功,但**管道几何产出为 0**,2638 件管件落 unknown、
>   1157 件落 catalog_pending。
> - 2026-08-31 11:44 用户:「结合 ida-bridge 继续分析 BRAN/HANG 的模型生成算法,
>   生成 plannator 开发文档,指导 e3d-model 的模型生成。」
>
> 权威(按证据强度排序):
> 1. **ida-bridge 活体反编译**(本计划 §2,本会话 11:4x–12:0x 实测)。实例:
>    `idalib-35724` = `D:\ida_scratch\plant3\Core3D.dll.i64`;
>    `idalib-41236` = `D:\ida_scratch\plant3\core.dll.i64`。凡标「活桥坐实」的都能按地址复查。
> 2. `.planning/2026-08-31-core-aligned-model-generation/task_plan.md`(算法线主计划,gate approved)。
>    本计划**继承**其 §1.1 内核边界、§1.2 实例化设计、§2.4 plug 全表、§2.4.1 旧路结论,不重复论证。
> 3. `上下文/会话-2026-08-31-core-dll模型生成活桥分析-7UW4.md`(7UW4 首轮活桥分析)。

## 与既有计划的关系

主计划 `2026-08-31-core-aligned-model-generation` 把工作分成 P0…P5,其中:

- **P4「目录件」** 只写了一行入口(`CSG_TreeBuilderCat::getCSGTree` 0x1072f5d0 待反编译),
  且**硬依赖**旧计划 `2026-08-30-direct-read-model-generation` 的 Phase 3(G4 目录表达式求值)。
- 主计划 §2.2 把 `MDR_Branch/SegmentVisualisationManager` 标成「1112 无管 —— **不做**」。

**本计划是 P4 的展开与前提修正**,做三件主计划没做的事:

| | 主计划现状 | 本计划 |
|---|---|---|
| 目录几何 | 只有一个待反编译的地址 | §2.4/§2.5 把 `CSG_TreeBuilderCat` + `CRCATI` **全表反编译完**(22 个几何类、19 个读参例程、逐个数字码) |
| MDR 分支 | 「1112 无管,不做」 | §2.2 **辨伪**:MDR 的 Branch 是 `RBRAN`(Marine 布线),**根本不是管道 BRAN**——「不做」这个结论对,但**理由**错了,不能据此认为管道已被覆盖 |
| BRAN/HANG | 未涉及 | §2.3/§2.6:管路与吊架**同构**,共用一套算法 |

**不改主计划文档**(避免与在飞 agent 撞);本计划的结论若被采纳,由 plannotator 门禁后回填主计划 P4。

---

## 一、目标

让 `vendor/e3d-model` 能对 **BRAN(管路分支)与 HANG(吊架)** 出几何:把二者的成员链
(目录件 + 隐式管身)按 core.dll 的语义解算成 CSG 树,几何内核仍走 `manifold-csg`
(主计划 §1.1 已定:core.dll 只作语义权威,不复刻 libgm)。

试点语料 = `D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001`
(6605 元素,管道族 PIPE 47 / BRAN 560 / FTUB 1538 / BEND 622 / ELBO 161 / ATTA 286)。

### 1.1 一句话结论(先看这个)

**BRAN 和 HANG 是同一种东西:一条「有序目录件链 + 两端点(HPOS/TPOS)」,
相邻件之间由隐式管身补齐。二者共用一套算法,不要写两遍。**
几何来源全部是**目录**:`ATT_GMRE → 几何集成员 → CRCATI 参数化基本体`,
其中隐式管身是这套基本体里的一个特例(`TUBE`,一个单位高圆柱)。

---

## 二、ida-bridge 实证底账

### 2.1 派发:BRAN/HANG 及其成员全部走旧路,现代 CSG builder 与它们无关

- 主计划 §2.4 已把 `CSG_TreeBuilder::addPlug`(0x107158b0)的**全部 12 个注册点**枚举完;
  该表里**没有** BRAN / HANG / PIPE / TUBI / FTUB / BEND / ELBO / VALV / ATTA 中的任何一个。
  唯一沾边的是 `CSG_PrimitiveUtilities::initialise` 挂的 `NOZZ` 与 `SUBCOM`,以及一串 `HV*`(HVAC)。
- 主计划 §2.4.1 已坐实派发口径:`CSG_TreeBuilder::getCSGTree`(0x10715b30)按
  `actualType() → type() → hardType()` 三键查 plug 树,**三键全 miss 就 `return 0`**;
  `GTGEOM`(0x10341d2e)据此落 `I*COM` 谓词级联走旧路。
- 本轮 xref 复核 `NOUN_BRAN`(0x10ae9158)/ `NOUN_HANG`(0x10ae93c4)的全部引用函数,
  未出现任何 `addPlug` 注册点(引用集中在 `FAB_*` 加工、`PDMS_*Element` 交互、`MDR_ApplicationManager` 编辑)。

**结论:** 管路/吊架的几何权威在旧路。`GTGEOM` 的谓词里 **`IPCOMP` / `IHCOMP`** 命中即走
`sub_10714FC0` → `create_geometry`(0x1071c3f0)复合递归。

> ✅ **第二轮已坐实,且原推断有一处错**(详见 `algorithm-bran-hang.md` §1.1):
> 这一族谓词全是「读 noun 字典属性」,不是硬编码判据 ——
> `IPCOMP`(core.dll 0x53a9a10)读 **`PIPC`** = Is **Piping** COMPonent(原推断正确);
> `IHCOMP`(0x53a9aac)读 **`HNGC`** = Is **HaNGer** COMPonent —— **不是 HVAC**,
> HVAC 另有谓词 `IHVCOM`(读 `HVAC` 字段)。
> 这个错若带进实现,会把吊架构件误当 HVAC 而整条漏掉。
> 同族还解出 `HTCOMP` 读 `HNGT`(= 路由容器标志)及另外 20 个谓词的字段名。

### 2.2 ★ 辨伪:`MDR_BranchVisualisationManager` 不是管道分支

主计划 §2.2 把 `MDR_Branch/SegmentVisualisationManager`(0x105e9aa0 / 0x105e9c80)标为
「1112 无管 —— 不做」,§2.4 又把 `MDR_CSGUtility::initialise`(0x105e9db0)挂的 noun 记为
「HATTA、TATTA(+Branch/AttachmentPoint/Segment 走间接 noun)」——**这三个间接 noun 本轮解出来了。**

`MDR_CSGUtility::initialise`(0x105e9db0,本轮反编译)从 `this[0]/this[1]/this[2]` 三个
运行时注入的 noun 建 plug;注入点是 `MDR_Manager::initialise`(0x105d0f50,本轮反编译):

```
MDR_CSGUtility::setBranchElement(u, NOUN_RBRAN)   // 分支 = RBRAN
MDR_CSGUtility::setAtta(u,         NOUN_RATTA)    // 附着点 = RATTA
MDR_CSGUtility::setSegment(u,      NOUN_RSEG)     // 段 = RSEG
MDR_CSGUtility::initialise(u)
    → addPlug(RBRAN, MDR_BranchVisualisationManager)
    → addPlug(RATTA, MDR_CSGAttachmentPoint)
    → addPlug(HATTA, MDR_CSGHeadAttachmentPoint)
    → addPlug(TATTA, MDR_CSGTailAttachmentPoint)
    → addPlug(RSEG,  MDR_SegmentVisualisationManager)
同一函数还注册:ATT_HPOS/ATT_TPOS、伪属性 MDR_DB_PseudoWVOLonRBRAN(ATT_WVOL on RBRAN)
```

**`RBRAN` 是 Marine 的「布线分支」(Routing BRANch),与管道 `BRAN` 是两个 noun。**
同一套 MDR 框架还被 `CAB_MDR_CableWayManager::initialise`(0x109b55e0)、0x109b61a0(CABLE)、
0x109f3950、0x109f9460 复用,各自注入自己的分支 noun ——它是**通用布线**框架,不是管道。

> **对主计划的修正:** §2.2 那一行「MDR_Branch/Segment —— 1112 无管 —— 不做」的**结论正确、
> 理由需改写**:不做的原因不是「1112 里没有管」,而是「它管的是 RBRAN 布线分支,与管道 BRAN 无关」。
> 若沿用旧理由,换到 ams8000(有 560 个 BRAN)时会误以为这个 builder 会接管,从而漏做本计划。

### 2.3 ★★ 隐式管身:用「负 refno」寻址,属主必须是 BRAN 或 HANG

`PDMS_Auto_ElementFactory::isTubiElement(int* refnoPair)`(0x10671cb0,本轮反编译):

```
if (refnoPair[0] >= 0) return false;                 // ★ 第一个字为负 = 这是隐式管身
elem = DB_Element(abs32(refnoPair[0]), abs32(refnoPair[1]));   // 取绝对值还原成真元素
if (elem.isNull() || hasHvacOwner(elem)) return false;         // sub_106681B0 = 属主链上有 HVAC
return elem.type()==NOUN_BRAN || elem.type()==NOUN_HANG
    || isPipingElement(elem) || isHangElement(elem);
```

配套判据(均本轮反编译):

| 判据 | 地址 | 定义 |
|---|---|---|
| `isPipingElement` | 0x106714d0 | `DB_Noun::getInt(actualType()) == 1` **且** 属主链上没有 HVAC |
| `isHangElement` | 0x10671210 | `DB_Noun::getInt(actualType()) == 1`(不排除 HVAC) |
| `hasHvacOwner`(`sub_106681B0`) | 0x106681b0 | `DB_Element::owner(elem, PredicateType(NOUN_HVAC))` 非空 |
| `isBranchElement` | 0x10670d60 | `type() == NOUN_BRAN` |

**两条可直接落地的结论:**

1. **隐式管身没有数据库记录。** 它以「前一个构件 refno 取负」的形式在管线里流通,
   还原时对两个字都取绝对值。e3d-model 的账本必须给它一类新身份
   ——它既不是 `visited` 的元素,也不是 `consumed` 的成员,而是**派生件**。
2. **`DB_Noun::getInt(noun) == 1` 是「管件族」的统一判据。** 管路与吊架的成员共享这一个族码。
   > ⚠️ `DB_Noun::getInt` 返回的是 dabacon 字典里的族码,**静态读不到**
   > (主计划已坐实 `DB_Noun` 静态镜像全 `0xffffffff`,码由字典运行时装)。
   > e3d-model 侧只能用**枚举白名单**替代,清单来源见 §6.1 第 2 条。

`PDMS_TubiElement` 侧的定位规则(0x10665430 / 0x10665730 / 0x10665800,本轮反编译):

- 管身的位姿取自它的**关联元素**:非首件时 = 该构件自身;首件时 = 分支 `firstMember` 的下一个
  (没有下一个就用 `firstMember` 自己)。
- 关联元素不存在时,位置退化为分支的 **HPOS**(`PDMS_HposElement::getHPOS`);
  若分支的某个引用属性的属主是 BRAN,则位置取原点。
- 朝向同理:关联元素不存在时取单位阵。

> ⚠️ 这三个函数属于 `PDMS_*Element` **交互层**(拾取/编辑/图形用),不是几何生成层。
> 它们坐实了「管身依附于哪个构件、端点从 HPOS 取」这层语义,但**管身的长度与实际两端点
> (arrive/leave)如何算,不在这里**,见 §2.7 与 §6.1 第 1 条。

### 2.4 ★ 目录几何的 CSG 组织:`CSG_TreeBuilderCat::getCSGTree`(0x1072f5d0,本轮反编译)

这是主计划 P4 只留了地址、本轮补全的那一个。伪码:

```
gmre = elem.getElement(ATT_GMRE)          // 目录几何集
if (!gmre.isValid()) return 0             // 没有几何集 = 不出几何(不是错误)
outer = gm_CreateCombination(options[31] ? 0 : 5)        // 5 = 并集
if (elem.hardType() == NOUN_SUBCOM)                       // 子组件
    transform = owner.getAtt(<变换>) * transform          // 把属主变换乘进来
for (m : gmre.members()) {                                // 逐个几何基元
    if (!options.isWanted(m)) continue
    m.elGoto()                                            // ★ 把 m 设为「当前元素」
    geom = CRCATI()                                       // §2.5:按 m 的 TYPE 造基本体
    if (!geom) continue
    tr = gm_CreateTransform(); gm_SetTransform(tr, xform_of(m))
    if (m.firstMember().isNull() || !options[28])
        gm_AddMember(geom, tr, outer)                     // 直接并入
    else {
        inner = gm_CreateCombination(options[31] ? 0 : 3) // 3 = 差集
        gm_AddMember(inner, identity, outer)              //   inner 并入 outer
        gm_AddMember(geom, tr, inner)                     //   基元 = 被减数
        CSG_TreeBuilderCat::addNegatives(inner, m, options)  //   减去 m 的负成员
    }
}
return outer
```

**落地要点:**

- **`ATT_GMRE` 是唯一入口。** 构件本身不带几何,几何在它指向的目录几何集上。
  没有 GMRE 就是「合法地没有几何」,应记账而不是 `failed`。
- **两层布尔:外层(op=5)聚合所有基元,内层差集(op=3)只作用在**有负成员的那个基元**上。**
  这跟结构板「整块板减所有洞」的形状不同,别照搬。
- `op=3 = 差集` 是主计划坐实的。**`op=5 = 并集` 这个推断第二轮被推翻:**
  扫遍 Core3D 全部 `gm_CreateCombination` 调用点后,取值分布是
  `0`×37(禁布尔时的分组)/ `1`×48(主流聚合)/ `2`×1 / `3`×22(差集)/ `4`×2 / `5`×5,
  并找到一对只差算子的孪生函数(`sub_1072BEA0` 用 5、`sub_1072C010` 用 1)——
  **1 与 5 是两个不同的聚合算子**,5 究竟是并集还是「装配不融合」未定。
  `options[31]` 置位时退化为 0 已坐实是「不做布尔、只分组」。
  **这条会影响体积对拍**(基元重叠时并集 ≠ 装配),验证方案见 `algorithm-bran-hang.md` §5.4。
- `elGoto()` 说明 **CRCATI 走的是「当前元素」全局态**,不是传参。移植成 Rust 时这是显式参数,
  但要注意 core.dll 里凡是 `DGETI/DGETR/YPARAM` 读的都是这个当前元素。

### 2.5 ★★ 目录基本体全表:`gml_bkend/CRCATI`(0x10345acc,本轮全文反编译)

`CRCATI` 用 `DGETI(TYPE)` 取当前元素的 noun 数字码后大 switch。
数字码 ↔ noun 名用 e3d-attlib 的 base-27 反哈希(`db1_dehash`,`k = code - 0x81BF1`)对上,
**22 个几何类、35 个 case 全部解出**,逐条可复查:

| 正 noun(码) | 负 noun(码) | 读参例程(pplib) | 造几何 | 备注 |
|---|---|---|---|---|
| SBOX 1014841 | NSBO 828671 | `QSBOX` 0x103bfcec | `GMCBOX(x,y,z)` | 负体**不**膨胀 |
| SREC 594640 | — | `QSRECT` 0x103b88bc | `gm_CreatePyramid(x1,y1,x1+dx,y1+dy,h,ox,oy)` | h 默认 1000,可被数据覆盖 |
| SANN 817255 | — | `QSANNU` 0x103b8e70 | `CRAPRO` 造 2D 环profile → `gm_CreateExtrusion(p,1000)`;有锥度/偏心时造第二个 profile 走 `gm_CreateRuledSolid(1000,p1,p2)` | **标称高 1000**,靠变换缩放 |
| SPRO 840259 | — | `QSPROF` 0x103b8410 | 取 profile 对 → 有第二个则 `RuledSolid(1000,p1,p2)`,否则 `Extrusion(p1,1000)` | 标称高 1000 |
| BOXI 726491 | NBXI 726152 | `QBOXI` 0x103bf6d8 | `GMCBOX(x, 1.0, z)` | Y 向标称 1.0 |
| **TUBE 631901** | **NTUB 586670** | `YPARAM(PDIA)` 0x103b8834 | `ORIMAT` 定向 + **`gm_CreateCylinder(PDIA/2, 1.0)`** | ★ **隐式管身**:半径 = P 点直径的一半,**高恒为 1.0**,长度由变换给 |
| SCON 818038 | NSCO 829400 | `QSCONE` 0x103bfe40 | `gm_CreateSnout(dx/2, dy/2, h, 0, 0)` | 负体 **h += 0.01** |
| SSPH 701101 | NSSP 860747 | `QSSPHE` 0x103c2480 | `gm_CreateSphere(d/2)` | |
| SDIS 912106 | — | `QSDISC` 0x103c25e0 | `gm_CreateCylinder(d/2, 1.0)` | 标称高 1.0 |
| LCYL 785955 | NLCY 1026041 | `QLCYLI` 0x103c0374 | `gm_CreateCylinder(d/2, h)` | 负体 **h += 0.01** |
| SCYL 785962 | NSCY 1026230 | `QSCYLI` 0x103bf3fc | `gm_CreateCylinder(d/2, h)` | 负体 **h += 0.01** |
| LSNO 837417 | NLSN 821192 | `QLSNOU` 0x103bffc4 | `gm_CreateSnout(dy/2, dx/2, h, ox, oy)` | 负体 **h += 0.01** |
| LINE 640317 | — | — | `CRCURV(1)` 0x103474c7 | 曲线,非实体 |
| SLINE 3471112 | — | — | `CRCURV(4)` | 曲线,非实体 |
| SCTO 841366 | NSCT 927815 | `QCTORU` 0x103c0e84 | `gm_CreateCircularTorus(max(r,0), R, 0, 角°)` | 弧度 ×57.29577951308232 |
| SRTO 841771 | NSRT 938750 | `QRTORU` 0x103c0fa0 | `gm_CreateRectangularTorus(r, R, h, 0, 角°)` | |
| SDSH 702883 | NSDS 908861 | `QSDISH` 0x103c0cbc | 三分支:`h>0 且 radius>0` → `CREDSH` 椭圆碟(0x1034ab41);`h>0` → `gm_CreateSphericalDish(d/2,h)`;`h<=0` → `gm_CreateCylinder(d/2,1.0)` | 需 `d>0` |
| SSLC 599770 | NSSL 782015 | `QSSLCY` 0x103c0514 | `CSRTAX` + `gm_CreateSlopeEndedCylinder(d/2,h,a3,a4,a1,a2)` | 四个角弧度→度后**规范到 (−90,90]**(>90 减 180,<−90 加 180) |
| LPYR 904404 | NLPY 1035518 | `QLPYRA` 0x103c07b4 | `gm_CreatePyramid(...)` | |
| SEXT 942751 | NSEX 1008005 | `QSSEXT` 0x103b7fcc | `gm_CreateExtrusionGroup(profileGroup, h)` | **`h < 1e-6` 直接不建**(静默跳过) |
| SREV 968617 | NSRE 643505 | `QSSREV` 0x103b81cc | `GMCCOM(1)` 组合 + `GMCTRA`;`\|角\|<1e-6` 视为 **360°**;超 180° 时按条件**拆成两段各半角**再 `GMAMEM` 合入 | `gm_CreateRevolution` |

**四条马上要改实现的结论:**

1. **★ 主计划 §2.3 的一处出入要翻案。** 主计划说 e3d-model 写的 `NSBO`/`NLCY`
   「在 core.dll 注册表里**根本不存在**」——那是**设计图元**注册表 `primList_`。
   本表坐实:`NSBO`(828671)、`NLCY`(1026041)**存在**,只是属于**目录基本体**这一族(CRCATI),
   不属于设计图元族(CRDESI)。**不要按主计划 P1 把它们删掉**,应改判为「目录负基本体」。
2. **★ 负基本体的 0.01 膨胀是有选择的。** 只有 `NSCO / NLCY / NSCY / NLSN` 四个在**长度**上 +0.01,
   `NSBO / NSSP / NSCT / NSRT / NSDS / NSSL / NLPY / NSEX / NSRE / NTUB / NBXI` **不膨胀**。
   照抄要按 noun 逐个抄,不能一刀切「所有负体都放大」。
   (这条对 manifold 尤其重要:膨胀是为了避开共面,漏抄会出共面差集的经典毛刺。)
3. **★ 标称尺寸 + 变换缩放是 core.dll 的原生做法。** TUBE 高 1.0、SDIS 高 1.0、
   SANN/SPRO 高 1000、BOXI 的 Y 向 1.0 —— 这直接印证主计划 §1.2 的实例化设计:
   **一份单位几何 + 实例变换**不是我们的发明,是照抄。§1.2.1 的缓存键设计可原样用于目录件。
4. **段数 N 仍由 `restol` 决定**,不在这张表里。§1.2.1 那条「N 必须进缓存键」的坑对目录件同样成立。

### 2.6 ★ HANG 与 BRAN 同构

`PDMS_HangElement`(本轮反编译 0x106724a0 / 0x106725f0 / 0x10672770 / 0x10672320):

| 方法 | 地址 | 语义 |
|---|---|---|
| `getHanger` | 0x106724a0 | 遍历 `owner().members()` —— 即该吊架下的**全部同级构件** |
| `nextElement` | 0x106725f0 | 下一个兄弟;**没有下一个且自身是 HANG 时返回 `PDMS_TposElement`**(尾点) |
| `prevElement` | 0x10672770 | 上一个兄弟;**没有上一个且自身是 HANG 时返回 `PDMS_HposElement`**(头点) |
| `getAssociatedObjects` | 0x10672320 | 吊架构件 + HPOS + TPOS |

**结论(本计划最省工的一条):** HANG 的拓扑 = 「HPOS(头) → 有序构件链 → TPOS(尾)」,
与 BRAN 逐点同构;§2.3 的 `isTubiElement` 也把 `NOUN_HANG` 与 `NOUN_BRAN` 并列接受。
**一套遍历器 + 一套管身补齐 + 一套目录求值,同时覆盖两者。** 不要为 HANG 另起分支。

### 2.7 目录寻址与 P 点:已定位的例程族(供 §6 按图索骥)

本轮把两个模块的例程表整个拉出来了(经 `MTRENT` 字符串反查),都在 Core3D.dll:

**`catdblib`(目录库寻址,SPRE → CATE → GMSE 这条链):**
`GATCAT` 0x1035c340、`G1TSPE` 0x1035c96c、`GATCRF` 0x1035d7d8、`GATGOC` 0x1035cfb0、
`GATGOP` 0x1035c25c、`GTGOPS` 0x1035c05c、`GATDDP` 0x1035d48c(设计维度参数)、
`GATGDP` 0x1035d38c、`G1TCTX` 0x1035dba4、`GTPINI` 0x1035bff4。

**`pplib`(P 点库,管身两端与构件连接口):**

| 例程 | 地址 | 用途 |
|---|---|---|
| `PARRIV` / `XPARRI` | 0x103bd1c4 / 0x103be8e4 | **到达点**(arrive) |
| `PLEAVE` / `XPLEAV` | 0x103bcef8 / 0x103be774 | **离开点**(leave) |
| `PARPOS` / `PLVPOS` | 0x103cd458 / 0x103cd7e8 | 到达/离开点的位置 |
| `IHEAD` / `TAIL` | 0x103bc8e4 / 0x103bcaec | 分支头/尾 |
| `QPPOS` / `QPDIR` / `QPBOR` / `QPCON` / `QPSHAP` | 0x103badac / 0x103bac30 / 0x103bbb2c / 0x103bc730 / 0x103bc818 | P 点的位置 / 方向 / 通径 / 连接型式 / 形状 |
| `QTUBE` / `PPOD` | 0x103bf5c0 / 0x103beb30 | 管身查询 / 外径 |
| `YPARAM` / `YPARUN` / `RPARAM` / `APARAM` / `MPARAM` / `SPARAM` / `DPARAM` | 0x103b8834 / 0x103c453c / 0x103b9478 / 0x103c577c / 0x103c1fa8 / 0x103c4bb4 / 0x103bf9c0 | 各类参数读取(`YPARAM(PDIA)` 即 §2.5 的管身半径来源) |
| `GTPLAX` / `PIPLAX` / `PLXVLD` / `NAXIS` | 0x103c5fb8 / 0x103c43f4 / 0x103c42a0 / 0x103bb4f4 | P 轴取法与校验 |

**这张表就是「管身长度怎么算」的答案所在地**:管身 = 前件 `PLEAVE` 到后件 `PARRIV` 之间,
半径由 `QPBOR`/`PDIA` 定。

> ✅ **第二轮已反编译到底,§6.1 第 1 条结案。** 全文见 `algorithm-bran-hang.md` §3,
> 关键例程 `GTTUBE` / `TUMAT` / `ORIMAT` / `CGETOD`。五条可直接照写的口径:
>
> 1. **长度 = `|P1 − P2|`**(`VDIST`,直线距离)—— **不扣端面、不做轴向投影**。
>    `P1` = 离开侧 P 点位置(容器头点用 `HPOS`,构件用目录侧 `LPOS`),
>    `P2` = 到达侧(容器尾点用 `TPOS`,构件用 `APOS`)。
> 2. **位置 = 两点中点** `(P1 + P2) · 0.5`(`TUMAT`)。
> 3. **朝向 = `ORIMAT(normalize(P1 − P2))`**;方向退化时**回落到 `D1`(离开侧 P 点方向)**,
>    不是取单位阵。`ORIMAT` 的副轴 tie-break 顺序(依次试 X̂/Ŷ/Ẑ)影响圆柱周向起点,
>    逐顶点对拍时必须照抄。
> 4. **半径 = `CGETOD`/2**:先取管子目录件 `PARA[2]`(外径),保温开启时加 `IPAR[1]`;
>    **取不到才回落到「到达侧」通径 `bore2`**。前后通径不一致时 core.dll 取的是到达侧,
>    别改成取离开侧或取平均。
> 5. **圆柱按真实长度直接建**(`gm_CreateCylinder(r, length)`)。
>    ⚠️ 别和 §2.5 混:那张表里 `TUBE`「高恒 1.0」说的是**目录基本体**那条路(CRCATI),
>    管身默认路不是「单位圆柱 + Z 向缩放」。
>    另外**实体只有圆柱一件**,`CLIN` 中心线与 `LINE` 线段不是实体,出网格时要跳过。
>
> 退化(零长 / P 点缺失 / 通径不一致)逐类记账不静默丢,清单见 `algorithm-bran-hang.md` §4。
> 口径同时覆盖 HANG(§2.6 已证同构),P2 据此解除封锁。

---

## 三、现状底账

### 3.1 e3d-model 在管道库上的实测(2026-08-31 08:19 探针,本会话复核 `out/ams8000/report.json`)

```
visited 6088 + consumed 517(LOOP 509 / PLOO 8)= 6605 = 索引总数   账平
generated 153(EXTR/BOX/CYLI/PANE)  failed 0  skipped 0  orphans {}  negatives_skipped 0
catalog_pending 1157 = BRAN 560 / SCTN 528 / PIPE 47 / FITT 22
unknown_nouns  2638 = FTUB 1538 / BEND 622 / ATTA 286 / ELBO 161 / TEXT 24 / HVAC 2 / STRT 2 / REDU 1 / VALV 1 / WELD 1
notes(唯一一条)= PANE 24384/26250 环 0 倒圆切点越界,按 E3D 近越界吸附口径处理
```

**定性:管线零缺陷,但管道几何 0 件。** 这不是 bug,是本计划要补的能力边界。

### 3.2 e3d-io 已有的地基(可直接复用,不要重造)

| 件 | 位置 | 状态 |
|---|---|---|
| 目录表达式解码(两种存储形式) | `record/catalogue_expr.rs`(18KB)、`record/catalogue_pml.rs`(26KB) | 已有,按 E3D `Q` 的口径渲染 |
| 目录表达式**求值**框架 | `record/catalogue_eval.rs` | 已有 `ParamEnv` trait(`param`/`iparam`/`design_dimension`/`attribute_number`)+ 测试用 `MapParamEnv`;镜像 core.dll `exprlib/EXEV*`、`exppdms/GATPAR`、`catdblib/GATINS` |
| ~~缺口~~ 目录表达式**求值**实现 | `db_param_env.rs`(新增)、`db_element.rs::get_number` | **已补**(见 §5 P1):`DbElementParamEnv` = 设计构件 + 目录件双宿主;`catalogue_pml` 补 `evaluate_words`(角度制);`AttributeNumber` 把「没这个点」与「算不了」分开记 |
| P 点轴规格 | `record/axis_spec.rs` | 已有:`PAXI/PBAX/PAAX/PCAX` 的 count-prefixed 词对解码,113343 元素普查 + 2026-08-27 E3D TTY 回读 35/35 一致 |
| 方向 / 点表 | `record/direction_spec.rs`、`record/point_list.rs` | 已有 |
| noun 反哈希 | `e3d-attlib::db1_dehash` | 已有,本计划 §2.5 的数字码就是用它对上的 |
| 跨库门面 | e3d-io `6bea669` S1 `DbElement` 门面 | 已有(目录在别的库,必须跨库取) |

### 3.3 主计划待回填的三处

1. §2.2「MDR_Branch/Segment 不做」的**理由**要改(见 §2.2)。
2. §2.3 说 `NSBO`/`NLCY` 不存在、P1 要删——**要翻案**(见 §2.5 结论 1)。
3. `IHCOMP` 若在主计划或任何笔记里被记成「Is **Hvac** COMPonent」,**要改成 Hanger**
   (见 §2.1)。HVAC 的谓词另有其人(`IHVCOM`)。

---

## 四、完成判据

- [x] `ams8000_0001` 全库跑完:`visited + consumed == 索引全集`,**且 `unknown_nouns` 中的
      管件族(FTUB/BEND/ELBO/ATTA/VALV/REDU/STRT/WELD)清零**——要么出几何,要么进有名目的账。
      2026-08-31 14:2x 实测:6088 + 517 = 6605 账平;8 族 2612 件全部进 `route_members`,
      `unknown_nouns` 从 2638 降到 26(只剩非路由的 TEXT / HVAC)。见 §5 P0 的实测表。
      注:此条只是「有名目」达标,**几何仍是 0 件**——那是 P1/P2 的事。
- [ ] BRAN 560 与 HANG(若语料中有)**共用同一条代码路径**;不允许出现两份遍历器。
- [ ] 隐式管身作为**派生件**单独记账:数量、总长、来源构件对,报告里可核对。
      负 refno 的识别与还原(取绝对值)有单测钉住。
- [ ] 目录基本体按 §2.5 的表**逐条实现**,每个几何类有单测(体积/AABB/连通分量);
      **负体膨胀按 noun 白名单**(只有 NSCO/NLCY/NSCY/NLSN 的长度 +0.01),有回归测试钉住。
- [ ] 目录几何的 CSG 形状按 §2.4:**外层聚合、内层差集只包住带负成员的那一个基元**。
      写一个用例专门证明它与「整体减所有负体」不等价时取前者。
      (外层聚合算子 `op=5` 的确切语义见 §6.1 第 4 条,P4 对拍前须定。)
- [ ] 表达式求值走 e3d-io 的 `ParamEnv`,**不在 e3d-model 里造第二套**。
      `PARAM n` 越界返回 `None`(GATPAR 的 223),不许兜底成 0。
- [ ] 实例化缓存(主计划 §1.2)覆盖目录件:键 = 目录几何元素 ref + 求解后的设计参数 + 段数 N;
      `--no-instance-cache` A/B 两条路产物**逐元素完全一致**;命中率进报告。
- [ ] RVM 对拍门(主计划 §4 的口径:体积 / AABB / 连通分量,不逐三角):
      管道子集逐元素比对,超差逐条归因。

---

## 五、阶段

### P0 — 分类表接住管件族,先把账做对(不出几何也要先有名目)

状态:**done**(2026-08-31 14:2x 落地并在 ams8000 实测)。不依赖任何逆向,已把 2612 个
管件族 unknown 收干净。

- [x] `category.rs` 新增 `Category::RouteContainer` / `Category::RouteMember` 接住路由族。
      实现:`data/route-nouns.json`(从本目录那份拷入 crate)经 `include_str!` 内嵌,
      `OnceLock` 解析一次成两个 `BTreeSet`;手抄表里没认领的 noun 才落到这条路上,
      再没有才算 `Unknown`。104 个短名一个都没写进 Rust 源码。
      ✅ **权威清单第二轮已产出,不必再猜、也不必退而求其次只收 10 个**:
      同目录 **`route-nouns.json`** —— 从 `vendor/e3d-io/catalog/e3d31/noun_layout.json`
      (活 E3D 导出)按**结构判据**筛出,而非人肉罗列:
      - **路由成员 104 个** = 同时带 `ARRI` + `LEAV` 两个属性的 noun(能进能出 = 在路由链上);
        ams8000 那 10 个 unknown 里,8 个管件族(FTUB/BEND/ATTA/ELBO/STRT/REDU/VALV/WELD)
        **全部命中,8/8**;剩下 `TEXT` 与 `HVAC` 判为非路由 noun,合理。
      - **路由容器 5 个** = 带 `HPOS` + `TPOS` 的 noun:
        `BRAN`(BRANCH)、`HANG`(HANGER)、`LUG`、`SUPC`(SUPCOMP)、`TRUNNI`(TRUNNION)。
        > 注意后三个:**吊架侧的容器不止 HANG**,LUG / SUPC / TRUNNI 同样是「有头有尾」的容器,
        > `RouteWalker` 的容器判据要按这 5 个来,只认 BRAN+HANG 会漏掉支撑件那一支。
      - 与 §2.5 `GTTUBE` 里 core.dll 硬编码的那批 noun 交叉验证一致。
      实现时按这个 JSON 生成表,别在 Rust 里手抄。
- [x] BRAN / HANG 归为路由容器、PIPE 归为纯组织节点(`List`)。
      **⚠ 本条的括号原文写错了,已按报告自证纠正**:原文说「现在 BRAN 落 catalog_pending
      就断了,560 个分支下面的成员根本没被走到」——**下钻从来没断过**。
      `pipeline.rs` 的 `push_members` 对每个元素无条件调用,只有「被属主消费的成员」例外,
      `Catalog` 从不在例外之列(模块文档第 16–19 行就写着这条是当年 CWALL 事故的修复)。
      自证:改动前那份 `out/ams8000/report.json` 里 FTUB 1538 / BEND 622 / ELBO 161 / ATTA 286
      **全部躺在 `unknown_nouns` 里**——它们正是 BRAN 的成员,进得了那本账就说明被遍历到了。
      真实缺陷只有一个:这 2612 件的**归属**是「分类表欠账」,不是「没走到」。
      照原文去修遍历会一无所获,而真正该改的分类表反倒不会被碰。
- [x] 报告新增账目。`catalogue_pending_by_noun` **本来就有**——`Report::catalog_pending`
      一直是 `BTreeMap<String, usize>`(报告里那行 `BRAN 560 / SCTN 528 / PIPE 47 / FITT 22`
      就是它),不必新开。实际新开的是三本:`route_containers`、`route_members`、
      `implied_tubes`(算法文档 §2.3 的字段口径,`total_length` 在 P2 出几何前是 `null`
      而不是 `0.0`——写 0 会被下游读成「量过了,总长是零」)。

验收:ams8000 上 `unknown_nouns` 中的管件族清零或逐条有归因;账仍平衡;不 panic。

**实测(2026-08-31 14:2x,`gen_ams` 全库,343 ms):**

```
visited=6088 consumed=517 generated=153 skipped=0 failed=0
catalog=550 route_containers=560 route_members=2612 implied_tubes=3172
unsupported=0 unknown=26 orphans=0 neg_skipped=0
账已平:visited + consumed = 索引 6605
```

| 项 | 改动前 | 改动后 |
|---|---|---|
| `unknown_nouns` | 2638(含 8 个管件族 2612 件) | **26**(只剩 `TEXT` 24 / `HVAC` 2) |
| `catalog_pending` | 1157(BRAN 560 / SCTN 528 / PIPE 47 / FITT 22) | 550(SCTN 528 / FITT 22) |
| `route_containers` | — | 560(BRAN) |
| `route_members` | — | 2612(FTUB 1538 / BEND 622 / ATTA 286 / ELBO 161 / STRT 2 / REDU 1 / VALV 1 / WELD 1) |
| `implied_tubes.count` | — | 3172 = 2612 + 560(每个 BRAN 一条尾管) |
| `generated` | 153 | **153**(逐 noun 一致:BOX 44 / CYLI 34 / EXTR 74 / PANE 1) |

`TEXT` 与 `HVAC` 留在 `unknown_nouns` 是**有意的**:两者按 `route-nouns.json` 判为
非路由 noun,没有证据支持把它们归到任何一类,按本 crate「宁可显式未知也不静默放行」
的口径显式挂着。要清掉得单独立据(`TEXT` 大概率是 `NonGraphic`,但没查)。

产物落 `vendor/e3d-model/out/ams8000-p0/`,基线 `out/ams8000/` 原样保留供对照。

> ⚠️ **并发**:`vendor/e3d-model` 无 git 且今天已发生过双会话同树写
> (`polyhedron.rs` 07:08、全 src 时间戳 11:29 被刷)。动手前先确认对方停手,
> 或先把主计划 P5 的「进 git」提前做掉。
>
> **已缓解(14:2x)**:仓已建 git,`master` 上有 3 个 commit(最新 `a27b1a7`),
> 本次改动落在工作区未提交。并发仍在:另一会话同期在改 `tests/increment_real.rs`
> (本次一个字未碰),它也曾在 14:17 观察到 `category.rs` 处于半改状态编译不过——
> 那正是本次编辑的中间态。有 git 之后这类互踩至少可回溯,但**仍然没有锁**。

### P1 — 目录几何求值最小闭环(单个构件出几何)

状态:**partial**(用户「开始实现」这一轮落地求值链;目录基本体尚未动)。依赖 P0。

- [x] **e3d-io**:实现 `DbElementParamEnv`,把 `ParamEnv` 的四个方法接到活元素。
      §3.2 点名的缺口已补。实测口径与几处**与原设想不同**的结论:

      1. **环境是「设计构件 + 目录件」一对,不是一个元素。** 全库数名字归属:
         `ANGL`/`RADI`/`DESP` 只在设计构件上,`PARA` 只在目录件上,两边不相交。
      2. **`PARAM n` 取目录件的 `PARA[n]`,不是设计构件的 `DESP[n]`。**
         `GATPAR` 文字说「实例的第 n 个设计参数」,字面像 `DESP`,实测否掉:
         `PTAX.PBOR` 全库 115 处存 `PARAM 1`,某 FTUB 的 `DESP[1]=0`(通径不可能是 0),
         而它指向的 `SCOM` 的 `PARA[1]=50`,与元件名里的 50 mm 对得上。
      3. **`ATTRIB RPRO <键>` 不是读属性,是查设计表。** `RPRO` 在 1933 个 noun 的
         字典里查无此名。真相:目录件 `DTRE → DTSE`,每个 `DATA` 成员以 `DKEY` 为键、
         `PURP` 说值从哪来(`DATA`→`PARA[NUMB]`、`DESP`→设计件 `DESP[NUMB]`、
         `ATTR`→设计件同名属性、`EXPR`→本行 `PPRO`)。实现上先认 `PPRO` 再认 `PURP`,
         两条读法在全库 4714 个端点上结果完全一致。
      4. 三角函数是**角度制**;`ATANT` 参数序未定、除零与非有限值一律 `None`。

      实测:2612 个路由构件走通 4714 个端点,定位属性**全部求出的 3185(67.6%)**;
      求不出的公式只剩 1 种——`( ATTRIB RPRO TLEN )`,1529 处,设计件全是 `FTUB`,
      其设计表 `TLEN` 行是 `PURP=EXPR` 而 `PPRO` 空,即 E3D 自己算。**不猜**
      (邻居 `HEIG` 读得到 1300/998,但两键标题不同,当成一回事是替目录作者编答案)。
      详见 `上下文/会话-2026-08-31-进展核对-BRAN-HANG收口-42024d02.md` 第三段。

- [ ] **钉死 `RPRO TLEN`**:`FTUB` 那 1529 处的几何全压在这一条上。
      要么反编 core.dll 里 `EXPR` 空槽的处理,要么在 E3D 里对一个 FTUB 查 `TLEN` 实测。
- [x] **把 `PPointResolver` 接到 `DbElementParamEnv`**:新模块
      `e3d-model/src/catalogue_point.rs`(`CataloguePoints`),走
      `SPRE →(CATR)→ SCOM → PTRE → PTSE` 取到 P 点,用 `DbElementParamEnv` 求
      `PDIS`/`PBOR`、用 `axis_spec::direction` 解 `PAXI`,再套 `world_matrix` 转世界系。
      `pipeline` 与 `gen_ams` 都换掉了 `CatalogueNotWired`;`gen_ams` 新增可重复的
      `--catalogue`(目录库与设计库同池,P 点在跨库的另一头)。

      **接上之后先自证,自证当场翻了车,这一条比接线本身重要:**

      新探针 `examples/tube_axis_check.rs` 判一条几何硬约束——管身沿连接轴生长,
      所以 `to.pos − from.pos` 必须与两端 P 点的轴**共线**。判共线不判同向:
      两端各自朝里朝外是 E3D 的约定问题,探针第一版按「出发端同向、到达端反向」
      写死,把一批全对的管误判成全错(`180.00°` 满屏),折进 `[0°, 90°]` 才对。

      1. **`PTCA` 的 `DIR` 是模板缺省值,不是方向。** 全库每个 `PTCA` 上它都是
         `[0, 1, 0]`。第一版拿它当方向:弯头的两个口解出**同一个**方向(都是局部 +Y
         转到世界系),而管身照建不误、长度也像模像样,只是朝向全错。
         `tubes_measured` 从 1 涨到 156 看着很好看,共线率只有 11%。
         **这种错法比解不出恶劣得多:记账上看不见,几何上看得见。** 已整类改回
         `None`,`measured` 落到 133 —— 数字变小,但剩下的每一条都站得住。
      2. **真方向在 `PTCD`,第三套字编码,还没解开。** 首字是含自身的总长:
         `P1 = [6, 5, 2, 21, 11, 61]`、
         `P2 = [17, 5, 2, 22, 12, 10, 1, 106, 2, 773119, 1, 1, 0, 1601, 1701, 15, 61]`。
         `catalogue_pml`(PML 逆波兰)与 `axis_spec`(轴规格)两个现成解码器都读不通。
      3. **`PTAX` 那条路是对的,有正面证据。** 分支 `/LV-CO-1R312-D` 的 ELBO 离开点
         解出 `(-4602.526, 7926.265, 2890)`,与容器 `TPOS` `(-4602.53, 7926.26, 2890)`
         差 0.005 mm。世界矩阵、`ORI` 解码、`PDIS` 求值、`PAXI` 解码这四样一起被这
         一条钉住了。

- [ ] **解开 `PTCD`**(`PTCA` 的方向编码)。全库 516 个 `PTCA` 端点全压在这一条上,
      与 `RPRO TLEN` 并列为 P1 剩下的两个硬缺口。
- [ ] **查清容器头与首件的横向偏移**:量出来的 133 条里,`容器头 → 构件到达`
      有 58/95 偏差 > 30°,且几乎全是「Z 对齐、XY 差几十到一百毫米」。
      样本 `/M-OR-1R312-C`:`HPOS = (-4563.92, 7992.03, 1950)`、`HDIR = +Z`、
      `HREF = 0/0`(头没接任何东西),而唯一的成员 FTUB `POS = (-4663.92, 7992.03, 1950)`
      ——整整差 100 mm 的 X。`构件离开 → 构件到达` 那一类共线率明显更高(14/30),
      所以问题多半在容器头这一侧,不在目录链。先别急着改,拿全库分布看是
      模型噪声还是系统性错读。
- [ ] **e3d-model**:`catalogue.rs` 新模块 —— 从构件 `ATT_GMRE` 取几何集,
      遍历成员,按 §2.5 的表造基本体,按 §2.4 的形状组 CSG(外并内差)。
- [ ] 目录基本体逐个实现,**先做 ams8000 实际用到的那几类**(用一个普查探针先数出来,
      别按表全做),其余留 `unsupported` 并记账。
- [ ] `SUBCOM` 的属主变换合成按 §2.4 处理。

验收:任取 10 个 ELBO/FTUB/BEND 手工核对几何(半径、长度、朝向)与 E3D 一致;单测覆盖每个已实现的几何类。

### P2 — 分支遍历与隐式管身

状态:**partial**(2026-08-31 15:5x 落地遍历器 + 几何核 + 记账;几何数据仍卡 P1)。
~~依赖 §6.1 第 1 条~~ → **算法层封锁已解除**(§2.7 给出四条公式),照 §2.7 写,
不要自行推导长度。

> ⚠️ **「可与 P1 并行」这句要收窄——本轮探针推翻了它的强形式。**
> 解除的是**算法**封锁,不是**数据**封锁。四条公式都要两个 P 点,而 P 点有两种来源:
>
> | 来源 | 属性 | 现在读不读得到 |
> |---|---|---|
> | 容器头 / 尾 | 容器自身 `HPOS/HDIR/HBOR`、`TPOS/TDIR/TBOR` | **读得到**,直读 |
> | 构件离开 / 到达 | 目录侧 `LPOS/LDIR/LBOR`、`APOS/ADIR/ABOR` | **读不到** |
>
> 设计元素上的 `LEAV`/`ARRI` 只是 **P 点编号**(ams8000 全库 2612 个成员实测清一色
> `LEAV=2 / ARRI=1`),坐标在目录里。本轮沿 `SPRE → SPCO.CATR → SCOM.PTRE → PTAX`
> 走通了取证链(实例 `13244/108794 SPCO → 13244/51902 SCOM → 13244/51859 PTSE →
> 13244/51862 PTAX`),结论是 **`PTAX` 的 `PDIS` 存的是目录表达式**——
> `/ACP1000-TFVL-P2` 里就是 `( ATTRIB RPRO TLEN )`,求值要 `DbElementParamEnv`,
> 即 P1 的第一件事。
>
> **实测比例:ams8000 的 3172 条槽位里,只有 1 条两端全落在容器属性上,其余 3171 条
> 至少一端要等 P1。** 所以 P2 能与 P1 并行的部分是「遍历器 + 几何核 + 记账」,
> 出几何那一步排在 P1 之后,不是并行关系。探针:`examples/route_probe.rs`。
>
> **后续(P1 求值链落地后):** 4198 个 `PTAX` 端点的 `PDIS` 已求出 2683 条,
> 516 个 `PTCA` 端点的 `PX/PY/PZ` 求出 502 条;合计 4714 个端点里 **3185(67.6%)**
> 定位属性全齐。求不出的只剩 `( ATTRIB RPRO TLEN )` 一种(1529 处,清一色 `FTUB`)。
> 也就是说 P2 的数据封锁已解开约六成七,余下的压在 §5 P1 那条 `TLEN` 待钉项上。
> 求值探针:`examples/ppoint_probe.rs`。

- [x] **几何核照 §2.7 落地**(`src/route.rs`,新模块):`orimat`(副轴 tie-break 顺序
      X̂→Ŷ→Ẑ 照抄)、`tumat`(中点 + 退化回落 `D1` 而非单位阵)、`tube_length`
      (`VDIST` 直线距离)、`tube_outer_diameter`(回落取**到达侧**)、`build_tube`
      (按真实长度直建圆柱,走 `csg_cylinder_solid` 的「轴向局部 Z、原点居中」约定,
      与 `TUMAT` 的中点平移正好配套)。13 条单测钉住,含 tie-break 顺序与回落方向。
- [x] **`RouteWalker` 落地**:`walk_route` 输出「容器头 →(构件)* → 容器尾」的槽位链,
      非路由成员记账但不串接、`prev` 不前移;链形单独抽成 `slot_indices` 以便不开库就测。
- [x] **P1 的缝显式化**:`PPointResolver` trait + `CatalogueNotWired` 默认实现。
      **不拿构件 `POS` 顶替目录 P 点**——顶上去能让 3171 条槽位「有几何」,长度却
      系统性偏短(P 点到构件原点那一段被吃掉),而报告一片绿。
- [x] **退化逐类记账**:`zero_length` / `missing_ppoint` / `bore_mismatch` 之外
      新增 `unbuildable`(两端解出来了但定不出朝向或取不到半径),把「还没接上」
      与「接上了但数据坏」分开,否则 P1 的进度会看起来比实际好。
- [x] `total_length` 改成**只有 `measured == count` 才给 `Some`**,另加
      `measured` / `measured_length_mm` 两个字段显示部分进度。量了一部分就报总长,
      写出来的数看着跟真的一样却是残的。
- [ ] **出几何(等 P1)**:`measure_tubes` 现在只量不建。管身没有库记录,塞进
      `GeneratedElement` 要先定死伪 refno 的形状,会连带动增量更新与 RVM 对拍的主键;
      目录链没接上前真能量出来的只有个位数,不值得为它现在就动那两处。
- [ ] 管身若自带目录几何集(`GATRF1(A, TUBI, GMRE)` 命中)走特例路:
      只认 `TUBE`/`BOXI`/`LINE` 三种 TYPE,且 `BOXI` 是 **Y 向吃长度**。
      先用探针数一下 ams8000 里这条路占比,不占比就先记账不做。
- [ ] `ZERO_LENGTH_TOL_MM` 目前取 libgm 面级容差 0.051mm 作占位——
      **`GTTUBG` 里那个 ε 的立即数本轮没反编译到**,是本模块唯一不是抄来的数,
      对拍阶段要钉死。

**实测(2026-08-31 15:5x,`gen_ams` 全库 ams8000,83 ms):**

```
visited=6088 consumed=517 generated=153 skipped=0 failed=0
catalog=550 route_containers=560 route_members=2612
implied_tubes=3172 tubes_measured=0 tubes_no_ppoint=3171
unsupported=0 unknown=26 orphans=0 neg_skipped=0
degenerate: zero_length=1 missing_ppoint=3171 bore_mismatch=0 unbuildable=0
账已平:visited + consumed = 索引 6605;3171 + 1 = 3172,槽位无一条静默消失
```

`elements.json` 与 `model.obj` 的 SHA256 与 P0 基线 `out/ams8000-p0/` **逐字节相同**——
本轮只动账不动几何,这是最硬的一条回归锚。那 1 条零长是 `24384/26204`:无名 BRAN、
`HPOS == TPOS ==` 原点、两端通径都是 0,一条没画完的空分支,E3D 同样不出管。

回归:lib **82**(P0 后 69 + 本轮 13)+ rvm_compare 6 + 真库门 1 全绿;
`cargo fmt --check` 与 `clippy --all-targets -- -D warnings` 干净。产物 `out/ams8000-p2/`。

- [ ] 一套 `RouteWalker`:输入**任一路由容器**(5 个:BRAN / HANG / LUG / SUPC / TRUNNI,
      见 §5 P0),输出「HPOS → (构件, 管身)* → TPOS」的有序序列。
      五者走同一实现(§2.6 已证 HANG 与 BRAN 同构)。
      遍历骨架照 `algorithm-bran-hang.md` §4 的伪码写,注意其中一条:
      **非路由成员要记账后继续下钻**(它可能自带几何),不是跳过;
      但它**不参与管身串接**,`prev` 不前移。
- [ ] 隐式管身:识别负 refno、取绝对值还原、按 `PLEAVE(前件) → PARRIV(后件)` 定两端,
      **严格照 §2.7 的五条**(直线距离 / 中点 / `ORIMAT` 且退化回落 `D1` /
      半径走 `CGETOD` 且回落取到达侧 / 圆柱按真实长度直接建)。
      别把它当「单位圆柱 + 缩放」写(那是目录基本体那条路)。
- [ ] 管身若自带目录几何集(`GATRF1(A, TUBI, GMRE)` 命中)走特例路:
      只认 `TUBE`/`BOXI`/`LINE` 三种 TYPE,且 `BOXI` 是 **Y 向吃长度**。
      先用探针数一下 ams8000 里这条路占比,不占比就先记账不做。
- [ ] 退化处理:零长管身、缺 P 点、前后通径不一致 —— 逐类记账,不静默丢
      (账目名照 `algorithm-bran-hang.md` §4 的表)。

验收:ams8000 的 560 个 BRAN 全部走完;管身数量与 E3D 的 TTY 导出对得上;账平衡。

### P3 — 实例化与性能

状态:proposed。依赖 P1 + 主计划 §1.2。

- [ ] 目录件按「几何集 ref + 求解后参数 + 段数 N」做缓存键;命中率进报告。
- [ ] 管身天然高命中(同通径同段数只有长度不同,长度走变换)——单独统计这一类的命中率。
- [ ] `--no-instance-cache` A/B 一致性测试进 CI。

验收:A/B 逐元素完全一致;命中率与省下的 mesh 次数可观测。

### P4 — RVM 对拍与收口

状态:proposed。依赖 P2。

- [ ] 按主计划 P5 的口径导出 ams8000 的 RVM,接 gen-model 现成对拍设施。
- [ ] 容差**先定后跑**;超差逐条归因。
- [ ] 覆盖矩阵:§2.5 的 22 个几何类逐个写终态(已实现 / 记账不做 / 语料未出现)。

---

## 六、待补逆向(都用现有活桥,不新起环境)

> **第二轮结果:6.1 全部四条 + 6.2 的第 6、7 条已结案,下沉到 §6.3。**
> 只剩一条真未决(`op=5`,已降级为「不挡开工、但挡对拍」),外加 6.2 第 5 条未做。

### 6.1 排在 P2 前(会决定实现口径)

1. ~~**★ 管身两端与长度的确切公式**~~ → ✅ **已解**,见 §2.7 的四条公式与
   `algorithm-bran-hang.md` §3。逐条回答原提问:起点 = 前件 `PLEAVE` 的 P 点位置,
   终点 = 后件 `PARRIV` 的 P 点位置;方向 = 两点差归一化(**不取 P 点自带的 `QPDIR`**);
   长度 = 两点直线距离,**不扣端面**;通径取**离开侧**(前件 `PLEAVE` 的 `PDIA`)。
   **P2 就此解除封锁。**
2. ~~**★ 「管件族」的权威 noun 清单**~~ → ✅ **已解**,走的是原方案②的加强版:
   不是普查目标库(会漏掉语料里没出现的),而是读 `noun_layout.json` 这份**活 E3D 导出**,
   按 ARRI/LEAV、HPOS/TPOS 的**属性结构**判定,来源可追溯。产物 `route-nouns.json`,详见 §5 P0。
3. ~~`IPCOMP` / `IHCOMP` 的展开与判据~~ → ✅ **已解且纠错**(§2.1):
   在 core.dll 本体里查到,是读 noun 字典字段 `PIPC` / `HNGC`;
   **`IHCOMP` = Hanger 不是 HVAC**。这条从「不改实现」升级成「必须改实现」。
4. **`op=5` 的确切语义(唯一仍未决的)。** `[31]` 已坐实 = 禁布尔只分组;
   `[28]` 的**行为**已坐实 = 「是否处理负成员」的开关(不置位则跳过内层差集、正体直接并入),
   只是它在 E3D UI 上对应哪个选项未知 —— 不影响实现,常规生成取 `[28]=true`、`[31]=false`。
   剩 `op=5` 与 `op=1` 的差别未定(§2.4)。
   **降级说明:不挡 P1/P2 开工**(单基元、不重叠时两者等价),
   但**挡 P4 体积对拍**,须在 P4 前用 `algorithm-bran-hang.md` §5.4 的方案验证:
   造一对故意重叠的基元,分别按并集与按装配算体积,与 E3D 的 `Q VOLUME` 对照取胜者。

### 6.2 排在 P1 前

5. `catdblib` 的 `GATCAT`(0x1035c340)/ `G1TSPE`(0x1035c96c)/ `GATCRF`(0x1035d7d8):
   从设计构件到目录几何集的确切寻址链(SPRE → CATE/SCOM → GMSE),
   确认 `ATT_GMRE` 是直存还是伪属性。
   **仍未做**——这是 P1 唯一的前置。(第二轮把相邻的 `GATREF`/`GATRFT` 解了,见 §6.3,
   但 SPRE → CATE 这一跳还没走。)
6. ~~`CSG_TreeBuilderCat::addNegatives` 的成员白名单~~ → ✅ **已解:根本没有白名单。**
   它无条件遍历 `m.members()`,凡 `options.isWanted` 通过的**全部**作为负体减去,
   与主计划 §2.5 板 builder 那 11 个 noun 的硬编码做法**不同**。
   实现时不要照搬板 builder 的白名单过滤。
7. ~~`ORIMAT` 与 `CSRTAX` 的矩阵构造~~ → ✅ **已解**(`algorithm-bran-hang.md` §4),
   与 e3d-io `axis_spec.rs` 的交叉验证仍建议在 P1 单测里补一道。

### 6.3 已结案,不必再查

- ~~MDR 的 Branch/Segment/AttachmentPoint 是哪三个 noun~~ → RBRAN / RSEG / RATTA(§2.2)。
- ~~`CSG_TreeBuilderCat::getCSGTree` 语义~~ → §2.4 全文反编译。
- ~~目录基本体有哪些、各自怎么造~~ → §2.5 全表。
- ~~HANG 要不要单独一套算法~~ → 不要,与 BRAN 同构(§2.6)。
- ~~管身两端点 / 长度 / 朝向公式~~ → §2.7(第二轮)。
- ~~管件族权威 noun 清单~~ → `route-nouns.json`(第二轮)。
- ~~`IPCOMP`/`IHCOMP`/`HTCOMP` 判据~~ → `PIPC`/`HNGC`/`HNGT`(第二轮)。
- ~~`addNegatives` 白名单~~ → 无白名单(第二轮)。
- ~~负 refno 是谁置的~~ → `GATRFT`(经 `GATREF` 调用):对 `NOUN_TUBI` 元素且 tube 标志置位时,
  把 `refno[0]` 取负后返回。这坐实了 §2.3 的负 refno 是**出口处生成**、非库内存储。
- ~~`CSG_TreeBuilderOptions[28]/[31]`~~ → [28] = 允许内层差集,[31] = 禁布尔只分组(第二轮)。

---

## 七、风险与依赖

- ~~**★ 最大的坑:把管件族 noun 靠印象列表。**~~ → **已拆弹**:`route-nouns.json` 有出处、
  按属性结构判定(§5 P0)。**残留纪律**:新增 noun 只能改那个 JSON 的生成口径,
  不许在 Rust 里手加一行——手加的那一行没有出处,就是这个坑的复活。
- **★ 新的头号坑:`IHCOMP` 的 Hanger/HVAC 误读。** 第二轮已纠(§2.1),
  但这个错很隐蔽:误当 HVAC 会让**整个吊架族静默不出几何**,而账面仍然平衡、报告不报错
  ——正是最难发现的那类失败。P0 落表时用 `route-nouns.json` 里 HANG 一支做一条断言钉死。
- **★ 负体膨胀不能一刀切。** §2.5 结论 2:只有四个 noun 在长度上 +0.01。
  统一放大会让所有目录差集都比 E3D 多切 0.01,体积对拍必飘;统一不放大则共面处出毛刺。
- **★ 隐式管身的记账身份。** 它没有数据库记录,既不能算 `visited` 也不能算 `consumed`,
  硬塞进现有五本账会让 `accounts_for` 判据失真。P0 就要给它开新账。
- **表达式求值不许兜底成 0。** e3d-io 的 `catalogue_eval` 注释已经写死这条纪律
  (「never a zero wearing the shape of an answer」),e3d-model 侧要保持一致:
  求不出就 `failed` 并留元素号。
- **目录在别的库。** ams8000 的构件指向目录库(跨库 ref),必须走 e3d-io 的跨库门面;
  目标库缺失时按「合法缺席」跳过并记账,不许崩。
- **manifold 的鲁棒性**(继承主计划 §7):目录件里薄壁、退化环、零长管身都会出现,
  失败落 `failed` 账并留元素号,不许静默出空。
- **并发写 + 无 git**(继承主计划 §7,且今天已实际发生两次):动手前确认对方停手,
  或先把「vendor 进 git」提前做掉。
- **主计划两处待回填**(§3.3),回填前别按旧文照做。

## 批注处置记录

| 轮次 | 批注 | 处置 |
|---|---|---|
| — | (待 plannotator 门禁) | — |

## Errors Encountered

| Error | Attempt | Resolution |
|-------|---------|------------|
| `.ida_scratch/ida_mcp_client.py` 连 127.0.0.1:13338 被拒 | 沿用 7 月的 IDA MCP 客户端 | 那条路已废;现在用 `ida-bridge` CLI(`C:\Users\dpc\.agents\tools\ida-bridge\.venv\Scripts\ida-bridge.exe`),`ida-bridge list` 看实例 |
| `names` 表按 `name IN ('NOUN_BRAN',…)` 查不到 | 直接用未修饰名 | 名字是 MSVC 修饰过的:`?NOUN_BRAN@@3QBVDB_Noun@@B`,要用 `LIKE '%NOUN_BRAN%'` |
| `strings` 表 `s.str` 报 no such column | 猜列名 | 列名是 `string_value`(还有 `address`/`length`/`type`) |
| 按 `name LIKE '%CR%'` 找 FORTRAN 例程,全是 `Create*` C++ 符号 | 关键词太宽 | FORTRAN 例程多数无符号;改用 `MTRENT` 字符串反查:`strings JOIN xrefs ON to_ea=address WHERE string_value LIKE 'pplib/%'`,一次拿全模块 |
| 数字码看不出是哪个 noun | — | base-27 反哈希:`k = code - 0x81BF1`,循环 `chr(k%27+64)` 后 `k/=27`(与 `e3d-attlib::db1_dehash` 同源) |
| CRCATI 里的常量地址读不出含义 | 直接看伪码 | 用 IDAPython 直读:`idc.get_wide_dword(addr)` 再反哈希 —— 0x10B4E114=TYPE、0x10B4E118=PDIA |
| 反哈希出 `ANARB` 这种不存在的 noun,且**结果看着像模像样**不易察觉 | 照记忆重写 `dec.py` 的 `dehash` | 漏了减 `0x81BF1` 偏移、字符顺序也反了。**教训:反哈希不要凭记忆重写**,照 `vendor/e3d-attlib/src/hash.rs` 的 `db1_dehash` 抄。已修 `dec.py` 并用已知 noun 双向自检(`h(dehash(x))==x`) |
| PowerShell 里 `ida-bridge list` 退出码 1 | 直接写带引号的完整路径 | 引号路径要加调用运算符:`& "C:\...\ida-bridge.exe" list` |
| 一行 Python 探 `noun_layout.json` 报 `unterminated string literal` | 在命令行里嵌套转义引号 | 别在一行里跟引号较劲,落成脚本文件 `.ida_scratch/probe_noun_props.py` 再跑 |
| `Get-ChildItem -Recurse` 找 JSON 挂住(后台跑到超时) | 全盘递归搜 | 杀掉后改为定向:已知目录 + `-Depth` 限深 |


## 2026-08-31 ida-bridge 实施进度（实测账本）

> 本节只记录已经运行并有产物的结果；“已分类”不计入“已建”。

- **字典基线**：已用 E3D 3.1 `attlib.dat`（5,840,896 bytes）重算 327 个级联 NOUN。19 家族口径下，动态分类已做到 `unknown_nouns=0`；其中 27 个进入已有 builder，300 个仍为 `EvidencePending`，不能算几何覆盖。
- **route member 分区**：2.10 为 55 IPCOMP + 30 IHCOMP + 7 个 2.10 缺失 + 12 无家族位；3.1 中这 7 个已进入 IPCOMP，因此为 62 IPCOMP + 30 IHCOMP + 12 无家族位。5 个 route container 仍独立记账。
- **IDA 类 28**：已确认 `GTGEOM -> sub_10714FC0 -> create_geometry`，以及 `sub_107189A0` 将 `NOZZ/ELCONN/EQUCOM` 映射为 28、IPCOMP/IHCOMP 映射为 29/30。`sub_107210A0` 会按成员顺序创建并串接 member builder。AMS8000 的 34 个 ELCONN 均无成员，故目前保持 `EvidencePending`，没有冒充已建。
- **AMS1112（sesno 722）**：索引 30,940；`visited=6059`、`consumed=24881`、`unknown=0`、`orphans=0`、`failed=0`。实际生成 4,613（PANE 4,275、GWALL 120、FLOOR 81、SBFI 137）。14 组 `WALL -> SPINE -> CURVE` 已按属主语义消费；目录待实现 637（其中短 DESP 的 SBFI 3 个已从错误账移入目录待实现账）。
- **AMS8000（sesno 264）**：索引 6,605；`visited=6088`、`consumed=517`、`unknown=0`、`orphans=0`、`failed=0`。3,172 个管身槽位中实际建成 562，零长度终态 2,590，缺 P 点 19，另有 target-not-found 8、unbuildable 1。逐元素缓存开/关结果完全一致（715 元素，规范 JSON SHA-256 `60f6ae4629259950f8c8ead91c47bcf1a05f1ed5c0726a4ef5cea0ffd2089094`）。
- **TLEN/PTCD**：PTCD 已按 Core `DORTXT` 的轴链 + PML 角表达式实现；`FTUB + RPRO TLEN` 仅在该 noun/key 组合下映射到实例 HEIG，依据旧 E3D/RVM 对拍中 `PXLE=TLEN=1500` 且实例 `HEIG=1500`。启用 TLEN 后缺 P 点由 1,545 降到 19，实建管身由 329 增到 562。
- **仍未过门**：AMS1112 的 637 个目录 builder、AMS8000 的 550 个目录/路由构件实体、8 个跨会话目录引用、RVM facet 的 only-baseline/only-generated 与体积/连通分量门，以及真实增量回归 721→722 少删除 42 个几何。上述项目仍是待实现/待对拍，不计入完成。
