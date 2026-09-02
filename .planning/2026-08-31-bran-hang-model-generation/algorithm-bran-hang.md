# 算法规格:BRAN/HANG 路由建模(可照写)

> 归属计划:`2026-08-31-bran-hang-model-generation`
> 版本:2026-08-31 第二轮(接力 M556 → 本会话)
> 状态:**算法层已闭环**——P2 的硬门槛(管身两端点与长度公式)已解出,不再是空白。
>
> 证据等级约定:
> - **[坐实]** = 本轮或前轮 ida-bridge 反编译原文可复查,给出地址。
> - **[交叉]** = 反编译结论与活体 E3D 导出(`e3d-io/catalog/e3d31/noun_layout.json`)
>   或 ams8000 实测数据独立吻合。
> - **[推断]** = 由结构推理得出,未直接反编译到判据;实现时按此写但要留验证钩子。
>
> 活桥实例:`idalib-35724` = `Core3D.dll`;`idalib-41236` = `core.dll`。

---

## 0. 一句话

**BRAN 与 HANG 是同一个东西:一条「头点 → 有序目录件链 → 尾点」的路由,
相邻两件之间由一根隐式管身补齐。**
一套遍历器 + 一套管身几何 + 一套目录求值,同时覆盖两者。

几何只有两个来源:

| 来源 | 谁产生 | 入口 |
|---|---|---|
| **目录件几何** | 链上每一个真实构件(ELBO/VALV/FLAN…) | `ATT_GMRE` → 几何集成员 → `CRCATI` 参数化基本体 |
| **隐式管身几何** | 相邻构件之间,**无数据库记录** | `cachegml/GTTUBG` → 圆柱**按真实长度直接建** + `TUMAT` 变换(§3.5) |

---

## 1. 词汇表:路由容器与路由成员

### 1.1 判据的真身 —— 它们是 noun 字典属性,不是硬编码 [坐实]

`core.dll` 的 `nounlib` 里那一族 `I*COM` 谓词,全部是「读某个 noun 的字典属性」:

```
IPCOMP(noun) = ATNINT(noun, PIPC)     0x53a9a10   管件构件
IHCOMP(noun) = ATNINT(noun, HNGC)     0x53a9aac   吊架构件
HTCOMP(noun) = ATNLOG(noun, HNGT)     0x53a8d58   路由容器(逻辑值)
```

`ATNINT` 走 `DB_Noun::findNoun` + `DB_Noun::getField`(0x58de8d0),字段值由字典**运行时装入**,
静态镜像读不出来。这就是主计划说「`DB_Noun::getInt` 静态读不到」的根因。

> **对主计划 §2.1 的修正:** 那里把 `IHCOMP` 猜成 "Is **Hvac** COMPonent"。
> 实际它读的是 `HNGC`,是 "Is **HaNGer** COMPonent"。HVAC 另有 `IHVCOM`(读 `HVAC` 字段)。
> 这个错如果带进实现,会把吊架构件误判成 HVAC 而整条漏掉。

同族全表(本轮一次性解出,`0x5D67840`/`0x5DB6B2C` 两片常量区):

| 谓词 | 字典属性 | 含义 | 谓词 | 字典属性 | 含义 |
|---|---|---|---|---|---|
| `IPCOMP` | `PIPC` | 管件 | `IHLCOM` | `HULC` | 船体 |
| `IHCOMP` | `HNGC` | 吊架件 | `IHVCOM` | `HVAC` | 暖通 |
| `HTCOMP` | `HNGT` | 路由容器 | `IJOCOM` | `JOIC` | 节点 |
| `IFCOMP` | `FRMC` | 框架 | `INGCOM` | `NGMC` | 负几何 |
| `ICABCO` | `CABC` | 电缆 | `IPFCOM` | `PFRC` | 型材框架 |
| `INCOMP` | `HOLC` | 开孔 | `IPLCOM` | `PLNC` | 板 |
| `ICCOMP` | `CATC` | 目录件 | `IPRCOM` | `PRFC` | 型材 |
| `ICOCOM` | `COMC` | 通用件 | `IPSKEY` | `PSKC` | P-skey |
| `IECOMP` | `EQUC` | 设备 | `IPTCOM` | `PTSC` | 点集 |
| `IFICOM` | `FITC` | 管配件 | `IG2COM` / `IGMCOM` | `GMTC` / `GMSC` | 几何 |

### 1.2 e3d-model 侧怎么替代 —— 用结构判据,不用印象 [交叉]

字典属性读不到,但**它表达的东西可以从属性面反推**,且有活体 E3D 的导出可查:

| 集合 | 结构判据 | 结果 | 数据出处 |
|---|---|---|---|
| **路由容器** | noun 同时拥有 `HPOS` 与 `TPOS` | **恰好 5 个:`BRAN` `HANG` `LUG` `SUPC` `TRUNNI`** | `noun_layout.json` |
| **路由成员** | noun 同时拥有 `ARRI` 与 `LEAV` | **104 个**(见 `route-nouns.json`) | 同上 |

**这两个集合不是猜的,而且被 core.dll 独立印证:**
`cachelib/GTTUBE`(0x1034352c)里硬编码了三个 noun 码走「头点用 `HSTU`」分支 ——
`BRAN`=808220、`TRUNNI`=137403155、`LUG`=537123 —— **三个全部落在上表那 5 个容器里**。
反过来,`HTCOMP` 读的 `HNGT` 就是「这个 noun 是路由容器」这一位。两条独立证据对上了。

落盘产物:`route-nouns.json`(本目录),含两份清单、GTTUBE 交叉核对结果、
以及 ams8000 那 10 个 unknown noun 的归属判定:

```
FTUB BEND ATTA ELBO STRT REDU VALV WELD  → route_member   (8 个,共 2612 件)
TEXT HVAC                                → not_a_route_noun(标注与暖通容器,本就不该进管路账)
```

> **P0 的「不许凭印象列 noun」这道门,现在有权威出处了。**

---

## 2. 隐式管身:身份、寻址、记账

### 2.1 它没有数据库记录 [坐实+交叉]

- 活体导出里 `TUBING`(短名 `TUBI`)标着 `isPseudo=true`、**属性个数 0**。
- ams8000 全库 6605 个元素的普查里**没有任何 TUBI 记录**。

两条独立证据:**管身是纯运行时派生件。**

### 2.2 负 refno 的出生地 [坐实]

前一轮只知道「负 refno 表示管身」(`isTubiElement` 0x10671cb0 判 `refno[0] < 0`)。
本轮找到了**谁把它变负的**:

`getattlib/GATRFT`(core.dll `0x5a368f0`,`GATREF` 0x5395ae0 的实现体)在解析
**引用型属性**时,若「管身标志」置位且解析出的元素类型是 `NOUN_TUBI`:

```c
if (tubeFlag && DB_Element::type(e) == NOUN_TUBI)
    out[0] = -DB_Ref::operator[](0);   // ★ 第一个字取负
else
    out[0] =  DB_Ref::operator[](0);
out[1] = DB_Ref::operator[](1);        // 第二个字永远不动
```

单值属性与数组属性两条路径**都有**这段(数组元素步长 20 字节)。

于是整条链闭合:

```
DB 层物化一个 TUBI 伪元素,它的 DB_Ref 借用「所依附构件」的 refno
   → GATRFT 交给 FORTRAN 层时把 refno[0] 取负作为标记
   → FORTRAN/图形层拿到负 refno
   → isTubiElement 判负 → 两字取绝对值 → 还原出 BRAN / HANG / 管件构件
```

这正好解释了 `isTubiElement` 的接受集合为什么是
`BRAN | HANG | isPipingElement | isHangElement` 而**不含 TUBI**:
还原出来的本来就不是 TUBI,是它依附的那个东西。

> [推断] 唯一未直接反编译到的一环:伪 TUBI 元素的 `DB_Ref` 确实等于所依附构件的 refno。
> 现有证据(两端行为 + TUBI 无记录)只支持这一种解释,但没有直接读到构造点。
> 实现时给这一步留断言:还原出的元素必须是路由容器或路由成员,否则记 `failed` 而不是静默跳过。

### 2.3 记账身份

管身**既不是 `visited` 也不是 `consumed`**,硬塞进现有五本账会让 `accounts_for` 判据失真。
必须单开一本 `implied_tubes`,字段:

```
count, total_length, by_owner_noun{},
degenerate{zero_length, missing_ppoint, bore_mismatch}
```

---

## 3. ★ 算法 B:隐式管身几何(P2 的硬门槛,已解出)

权威:`cachegml/GTTUBG`(0x10340f8e)+ `cachelib/GTTUBE`(0x1034352c)+ `cachelib/TUMAT`(0x103439c0)。
三个都是本轮全文反编译。

### 3.1 取两端点 —— `GTTUBE` [坐实]

输入:`A` = 管身所依附的元素(链上前一件,或链首时是容器本身)。

```
DGOTO(A);  tA = A.TYPE

── 起点(离开侧)────────────────────────────────────────────
if  tA == BRAN  or  HTCOMP(tA)              // A 是路由容器 → 这是「头管」
      IHEAD()  →  P1 = A.HPOS,  D1 = A.HDIR,  bore1 = A.HBOR,  conn1 = A.HCON
      stub = (tA ∈ {BRAN, TRUNNI, LUG}) ? A.HSTU : A.HSRO
      DBEFOR()
else                                         // A 是普通构件 → 常规管身
      PLEAVE() →  P1 = cat(A).LPOS, D1 = cat(A).LDIR, bore1 = cat(A).LBOR, conn1 = cat(A).LCON
      stub = IPCOMP(tA) ? A.LSTU : A.LSRO
      DDEST(-1); CRETUR()

── 跳到后件 ───────────────────────────────────────────────
NATTA(NEXT);   B = 当前元素;   tB = B.TYPE

── 终点(到达侧)────────────────────────────────────────────
if  tB == BRAN  or  HTCOMP(tB)              // B 是容器 → 这是「尾管」
      TAIL()   →  P2ᴮ = B.TPOS, D2ᴮ = B.TDIR, bore2 = B.TBOR, conn2 = B.TCON
else
      PARRIV() →  P2ᴮ = cat(B).APOS, D2ᴮ = cat(B).ADIR, bore2 = cat(B).ABOR, conn2 = cat(B).ACON

── 换算到 A 的坐标系 ──────────────────────────────────────
M = CSTRAM(A, B)  =  inv(World(A)) · World(B)
P2 = M · P2ᴮ        (TRAVEC,含平移)
D2 = R(M) · D2ᴮ     (MVMULT,只转不移)
```

**要点:**

- `LPOS/LDIR/LBOR/LCON` 与 `APOS/ADIR/ABOR/ACON` 是**目录侧**的值,
  由 `GATPOS/GATDIR/GATREA/GATWR1` 经元素的 `REF`(SPRE)解出;
  设计元素上的 `LEAV`/`ARRI` 只是**P 点编号**,选中是哪一个 P 点。
- `HPOS/HDIR/HBOR/HCON`、`TPOS/TDIR/TBOR/TCON` 是**容器自身**的属性,`DGETRA/DGETR/DGETI` 直读。
- `IHEAD` 固定 P 点号 = 1,`TAIL` 固定 = 2。
- 读不到时的兜底 [坐实]:`PLEAVE`/`PARRIV` 出错 → 位置 `(0,0,0)`、方向 `(0,0,1)`、通径 0、
  并把 ok 标志清零;`IHEAD`/`TAIL` 遇错误码 18 同样退化。**退化要记账,不要当成合法零长管。**

### 3.2 定位与定向 —— `TUMAT` [坐实]

```
TUMAT(P1, D1, P2)  →  4×3 双精度矩阵 M(行 0/1/2 = 局部 X/Y/Z 轴,行 3 = 平移)

M.translation = (P1 + P2) · 0.5              ← ★ 中点,常量 0.5 已从 0x10B4DC10 读出
v = normalize(P1 − P2)
M.rotation = ORIMAT( v 退化时用 D1 )
```

### 3.3 定向矩阵 —— `ORIMAT`(core.dll `0x526d674`)[坐实]

```
Z = normalize(dir)                       若退化 → 报错返回
for e in (X̂, Ŷ, Ẑ):                     ← 按 X、Y、Z 顺序试
    X = normalize(Z × e)
    if 不退化: break
else: 报错
Y = Z × X
矩阵行序 = [X; Y; Z]
```

**这条决定了圆柱的周向起始位置。** 体积/AABB 对拍用不上,
但要逐顶点复刻 E3D 就必须照抄这个 tie-break 顺序,不能随手取任意垂直向量。

### 3.4 半径 —— `CGETOD`(pplib `0x103bf7ac`)[坐实]

```
od = GATREA(A, TUBI, PARA, index 2)          ← 管子目录件 PARA 数组的第 2 个元素 = 外径
if 保温开关(GATINS 置位):
    od += GATREA(A, TUBI, IPAR, index 1)     ← 保温参数数组第 1 个
if 上面取不到:
    od = bore2                               ← ★ 回落到「到达侧通径」,不是离开侧
半径 = od / 2
```

> 注意回落取的是 `P2` 侧的 `ABOR/TBOR`。前后通径不一致时 core.dll 就是这么选的,别改成取前侧或取平均。

### 3.5 造几何 [坐实]

```
length = |P1 − P2|                                    ← VDIST,直线距离,不扣端面
outer  = gm_CreateCombination(5)

若 A 的管子目录件有几何集(GATRF1(A, TUBI, GMRE) 命中):
    遍历几何集成员,只认三种 TYPE:
      TUBE 631901 → QTUBE 读 PDIA 与 PAXI → cylinder(PDIA/2, length),
                     变换 = TUMAT 矩阵,平移再叠加「PAXI 偏移经 A 的世界矩阵旋转」后的量
      BOXI 726491 → QBOXI 读 x、z        → box(x, length, z)          ← Y 向吃长度
      LINE 640317 →                        线段(P1→P2),非实体
    每个成员先过显示过滤(LEVE[2] / OBST / TVIS / BVIS / CLFL / TUFL)

否则(绝大多数情况,走默认路):
    cylinder(CGETOD/2, length),变换 = TUMAT 矩阵      ← TUFL=true, CLFL=false
    外加一条 CLIN(813891)中心线,线段 P1→P2            ← CLFL=true, TUFL=false,非实体
```

**`length` 直接进 `gm_CreateCylinder(r, h)` 的高度参数** ——
所以 CRCATI 里 `TUBE` 那条「高恒为 1.0」说的是**目录基本体**那条路;
默认路的圆柱是直接按真实长度建的,不是单位圆柱再缩放。这两条别混。

**实体只有圆柱一件。** `CLIN` 是中心线、`LINE` 是线段,都不是实体,
出网格时应跳过(否则会污染连通分量与体积对拍)。

---

## 4. 算法 A:路由遍历(RouteWalker)

```
walk(container):                       # container ∈ {BRAN, HANG, LUG, SUPC, TRUNNI}
    members = container.members()      # 有序
    prev = container                   # 头点由容器提供
    for m in members:
        if not is_route_member(m):     # 不在 104 个 ARRI/LEAV noun 里
            记账 non_route_member,继续下钻(它可能自带几何)
            continue                   # ★ 不参与管身串接,prev 不前移
        yield ImpliedTube(from=prev, to=m)
        yield CatalogueComponent(m)
        prev = m
    yield ImpliedTube(from=prev, to=container)   # 尾管,终点取 container.TPOS
```

`PDMS_HangElement`(0x106725f0 / 0x10672770)已坐实:`nextElement` 走到头返回 `TposElement`,
`prevElement` 走到头返回 `HposElement` —— 与上面的首尾闭合方式一致。
**HANG 不需要单独一套。**

退化情形逐类记账,不静默丢:

| 情形 | 处置 |
|---|---|
| `length < ε` | 记 `implied_tubes.degenerate.zero_length`,不建几何 |
| P 点取不到(ok 标志为 0) | 记 `missing_ppoint`,不建几何 |
| `bore1 ≠ bore2` | 记 `bore_mismatch`,按 §3.4 取到达侧,照常建 |
| 还原出的元素不是路由 noun | 记 `failed` 并留元素号(见 §2.2 的断言) |

---

## 5. 算法 C:目录件几何

### 5.1 CSG 组织 —— `CSG_TreeBuilderCat::getCSGTree`(0x1072f5d0)[坐实,前轮]

外层聚合所有基元;**内层差集只包住「带负成员的那一个基元」**,不是「整块减所有洞」。
结构板 builder 的写法不能照搬。

### 5.2 负成员 —— `addNegatives`(0x1072f480)[坐实,本轮]

前一轮把它列为「待查:成员白名单是什么」。**答案是没有白名单:**

```c
for (m : positivePrimitive.members()) {
    if (!options.isWanted(m)) continue;
    m.elGoto();
    geom = CRCATI();                 // 同一张基本体表,不另开分支
    gm_AddMember(geom, transformOf(m), differenceCombination);
}
```

正基元的**每一个成员**都是负体,用**同一个 `CRCATI`** 造,只被 `isWanted` 过滤。
这跟结构板 builder 那 11 个 noun 的白名单是两种做法 —— §6.2 第 6 条结案。

### 5.3 基本体全表

见计划 `task_plan.md` §2.5(22 类、35 个 case,含各自 pplib 读参例程与 `gm_Create*` 造法)。
本轮没有改动那张表,只补两条:

- `TUBE` 的读参例程精确为 `QTUBE`(0x103bf5c0):`直径 = |YPARAM(PDIA)|`,
  轴向由 `NAXIS(PAXI)` 给,取不到时方向退化为 `(0,0,1)`、位置 `(0,0,0)`。
- 负体膨胀白名单不变:只有 `NSCO / NLCY / NSCY / NLSN` 在长度上 `+0.01`。

### 5.4 ★ 组合算子:主计划的推断要改 [坐实]

主计划与前一轮计划都写着「`op=5` 疑为并集」。本轮把 Core3D 里
`gm_CreateCombination` 的**全部调用点**扫了一遍,取值分布是:

| op | 出现次数 | 现场 |
|---|---|---|
| 0 | 37 | `byte_10E5072C`(禁布尔开关)置位时一律退化成它 → **纯分组,不做布尔** |
| 1 | 48 | 最主流的聚合算子 |
| 2 | 1 | `CSG_TreeBuilderCLNTIL` |
| 3 | 22 | `addHolesBelowTemplate` / `addStandAloneNegative` / `addCutPlanes` → **差集**(已坐实) |
| 4 | 2 | — |
| 5 | 5 | `CSG_TreeBuilderCat` 外层、`GTTUBG` 外层,等 |

并且找到一对**只差算子、其余完全相同**的孪生函数(`sub_1072BEA0` 用 5,`sub_1072C010` 用 1),
说明 **1 和 5 是两个不同的聚合算子,不是同一个**。

**诚实结论:`op=5` 究竟是并集、还是「聚合但不融合」的装配语义,本轮没有定论;**
`gm_CreateCombination` 由 `libgm.dll` 导出,而活桥没有加载 libgm 实例
(文件在 `E:\reverse\e3d\shadow_e3d31_aps_all\libgm.dll`)。

**为什么这条不能含糊:** 目录件的多个基元若重叠,「布尔并集」与「装配分组」的**体积不同**。
主计划 §4 的对拍口径正是体积 / AABB / 连通分量。所以:

> P1 实现时先按并集做,但**必须**在报告里单列「基元间存在重叠的目录件」计数;
> 若这类件在 RVM 对拍中系统性偏大,即为 op=5 应作装配语义的证据。
> 或者直接加载 libgm 实例把 `GM_Operation` 枚举读出来,一次了结。

---

## 6. 坐标系与矩阵原语

| 原语 | 地址 | 语义 |
|---|---|---|
| `ORIMAT` | core `0x526d674` | 方向向量 → 3×3 行序 `[X;Y;Z]`,规则见 §3.3 |
| `TUMAT` | Core3D `0x103439c0` | 两点 → 4×3 矩阵(中点 + `ORIMAT`) |
| `CSTRAM` | core `0x51e393b` | `inv(World(A)) · World(B)`;A、B 同一元素时返回单位阵 |
| `TRAVEC` / `MVMULT` | — | 点变换(含平移)/ 向量变换(只转) |
| `VDIST` | — | 两点直线距离 |
| `MUNIT` / `VUNIT(v,k)` | — | 单位阵 / 轴单位向量(k: 0=零向量, 1=X̂, 2=Ŷ, 3=Ẑ) |

矩阵内存布局:**4 行 × 3 双精度 = 96 字节**,行 0/1/2 是旋转,行 3(字节偏移 72)是平移。

---

## 7. 属性字典(本轮 base-27 反哈希解出,可复查)

反哈希算法与 `e3d-attlib::db1_dehash` 同源:`k = code − 0x81BF1`,循环 `chr(k%27+64)` 后 `k/=27`。
自检:`TUBE ↔ 631901`、`PDIA ↔ 557809` 往返一致。

> 前一轮解出「ANARB」这类乱码,原因是 `.ida_scratch/dec.py` 里的 `dehash` 漏减 `0x81BF1`
> 且字序反了。本轮已修正该文件,并新增 `NH(ea, n)` 直接把内存里的常量按名字打印。

| 常量区 | 用途 | 解出的名字 |
|---|---|---|
| `0x10B4D8A0`… | GTTUBG | `5`(算子) `REF` `TUBI` `GMRE` `NEXT` `TYPE` `LEVE` `OBST` `TVIS` `BVIS` `CLFL` `TUFL` |
| `0x10B4DBD4`… | GTTUBE | `TYPE` `LSTU` `LSRO` `LHEA` `HSTU` `HSRO` `NEXT` `LTAI` `REF` |
| `0x10B6F0E0`… | PLEAVE | `LEAV` `REF` `TYPE` `LPOS` `LDIR` `LBOR` `LCON` |
| `0x10B6F118`… | PARRIV | `ARRI` `REF` `TYPE` `APOS` `ADIR` `ABOR` `ACON` |
| `0x10B6F074`… | IHEAD | `TYPE` `HPOS` `HDIR` `HBOR` `HCON` |
| `0x10B6F09C`… | TAIL | `TYPE` `TPOS` `TDIR` `TBOR` `TCON` |
| `0x10B6F368`… | QTUBE | `PDIA` `PAXI` |
| `0x10B6F3B4`… | CGETOD | `TUBI` `PARA`(下标 2) `IPAR`(下标 1) |
| `0x5D67840`… / `0x5DB6B2C`… | nounlib 谓词族 | 见 §1.1 表 |

noun 码:`BRAN`=808220、`HANG`、`TUBE`=631901、`BOXI`=726491、`LINE`=640317、
**`CLIN`=813891**(中心线)、`PLIN`=813904(折线)、`TRUNNI`=137403155、`LUG`=537123。

---

## 8. 数组属性下标语义(顺带解出,P1 会用到)[坐实]

`GATRFT` 里那段下标规范化,对所有 `GAT*` 数组读取通用:

```
n = 数组长度
若 last == 0        → first = 1, last = n          (取全部)
若 last  > 0        → last = min(last, n)
若 last  < 0        → first += n+1;  last += n+1   (负下标从尾部数,-1 = 最后一个)
若 first > n 或 first <= 0 → 错误码 223
```

**223 就是 e3d-io `catalogue_eval.rs` 注释里点名的那个「`PARAM n` 越界」错误码。**
两边口径一致:越界返回 `None`,不许兜底成 0。

---

## 9. 与既有文档的出入(需回填)

| # | 文档 | 原文 | 应改为 |
|---|---|---|---|
| 1 | 本计划 §2.1 | `IHCOMP` 疑 = Is **Hvac** COMPonent | 读 `HNGC` = Is **HaNGer** COMPonent;HVAC 是另一个谓词 `IHVCOM` |
| 2 | 本计划 §2.4 / 主计划 | `op=5` 疑为并集 | 1 与 5 是两个不同聚合算子;5 的确切语义未定,见 §5.4 的验证方案 |
| 3 | 本计划 §6.2 第 6 条 | `addNegatives` 的成员白名单待查 | **没有白名单**,见 §5.2,结案 |
| 4 | 本计划 §6.1 第 1 条 | 管身两端与长度公式未反编译,**P2 不许动工** | **已解出**(§3),P2 解除封锁 |
| 5 | 本计划 §6.1 第 2 条 | 管件族权威清单无出处 | 已有出处,见 §1.2 与 `route-nouns.json` |
| 6 | 本计划 §6.1 第 3 条 | `IPCOMP`/`IHCOMP` 展开名未坐实 | 已坐实,见 §1.1 |
| 7 | 主计划 §2.2 | MDR Branch「1112 无管,不做」 | 结论对、理由错:MDR 管的是 `RBRAN` 布线分支(前轮结论,仍成立) |
| 8 | 主计划 §2.3 | `NSBO`/`NLCY` 不存在,P1 删除 | 存在,属目录基本体族(前轮结论,仍成立) |

---

## 10. 仍未坐实的(不影响动工,但要留钩子)

| # | 事项 | 影响面 | 建议 |
|---|---|---|---|
| 1 | `op=5` 的确切语义 | 目录件重叠基元的体积 | §5.4:先按并集做 + 单列重叠计数;或加载 libgm 实例读枚举 |
| 2 | `CSG_TreeBuilderOptions` 的 `[28]`/`[31]` 的**命名意图** | `[31]` 已知是「禁布尔」总闸(与 `byte_10E5072C` 同义);`[28]` 的**行为**已坐实 = 「是否处理负成员」的开关(不置位则跳过内层差集、正体直接并入),但它在 E3D UI 上对应哪个选项未知 | 行为足够照写:常规几何生成取 `[28]=true`、`[31]=false`。命名意图不影响实现 |
| 3 | 伪 TUBI 元素的 `DB_Ref` 等于所依附构件 refno | 管身归属 | §2.2:实现里加断言,不成立就 `failed` |
| 4 | `catdblib` 的 `GATCAT`/`G1TSPE`/`GATCRF` 寻址细节 | 从设计件到 `GMRE` 的完整链路 | 接口已知(`GATRF1(elem, TUBI, GMRE)`),内部展开留到 P1 实测时对齐 |
| 5 | `CSRTAX`(仅 `SSLC` 用) | 一个基本体类 | 低优先 |
