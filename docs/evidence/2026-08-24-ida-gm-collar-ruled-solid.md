# 2026-08-24 IDA：`GM_Collar`（`gm_CreateRuledSolid`）逐位读出

`docs/evidence/2026-08-24-ida-occ-retire-audit.md` 的续篇。那一轮把 14 个形状臂里
13 个的 libgm 实现都复核过了，剩下 `GM_Collar` ——**斜切墙那一支（T022 / T023）用的就是它**，
而此前手上只有 `teach/learning-records/0011-libgm-geometry-algorithms.md` §5.14 一行
2.10 版的转述（「对上下两个轮廓的每一对 span 取半径较大者，两端强制同段数」）。这一刀把
3.1 版逐位读出来，结论比那行转述多出四件事。

方法：长驻 idalib 实例 `idalib-18608`（libgm 3.1，`D:\AVEVA\Everything3D3.1\libgm.dll.i64`），
全程只读（pseudocode / names），未写 idb。

## 地址表（libgm 3.1，ImageBase `0x10000000`）

| 符号 | 3.1 | 2.10（teach 0011 口径） |
|---|---|---|
| `GM_Collar::GM_Collar(double, GM_Profile*, GM_Profile*, double)` | `0x10048390` | — |
| `GM_Collar::calcFacetsWithoutSurfaces` | `0x10048500` | `0x100299e0` |
| `GM_Collar::formBaseFacet` | `0x10048d60` | — |
| `GM_Collar::formTopFacet` | `0x10048f20` | — |
| `GM_Collar::formTopSides` | `0x100490d0` | — |
| `GM_Collar::validate` | `0x10049290` | — |
| `GM_Collar::linkedProfiles` | `0x10049340` | — |
| `GM_Collar::setSpanSteps` | `0x100498c0` | `0x1002b3c0` |
| `GM_Collar::otherEnd` | `0x100484a0` | — |
| `GM_Profile::polygonForFacet` | `0x1008ed80` | — |
| `GM_Profile::setNSteps(FL_vector<int>&)` | `0x1008f130` | — |
| `GM_Profile::setNSteps(double) const` | `0x1008f2e0` | — |
| `GM_Profile::getPolygonForFacet` | `0x1008f8b0` | — |
| `GM_Profile::getNFacetsRoundProfile` | `0x1008ecb0` | — |

构造签名由 ctor（`0x10048390`）坐实：`GM_Collar(height, baseProfile, topProfile, tol)`，
两个轮廓各包一层 `GM_ProfileInst` 挂进 `this+0x14` 的实例链，base 在 `+8`、top 在 `+12`。
高度存 `this+0x28`。与 ADR-030 记的 `gm_CreateRuledSolid(len, profA, profB)` 一致。

## 一、`validate`（`0x10049290`）——「两端点数一一对应」是**前置条件**，不是算法

```c
if (height <= 1e-6)                     code = -88
else if (top.nBulges != base.nBulges)   code = -61     // 硬拒
else if (base.state != 4)               code = -99
else                                    code = (top.state == -50) ? -63 : 1
```

比较的是 `D2_Profile` 里 `+52..+56` 那个 8 字节步长数组的长度（每跨度一个 bulge），
即**两端轮廓的跨度数**。取 base/top 之前会按需触发 `state == 2 → 虚表 +32`（延迟校验）。

本仓 `sweep_mesh::loft_loops` 的文档写着「两端点数一一对应」，此前是个未署名的假设；
现在它有权威出处，而且方向更强：**不满足时 libgm 整条拒建（−61），不是靠插值凑合**。
高度门 `1e-6` 与错误码 −88 和 `GM_RectTorus::validate` 的负高同码，编号族对得上。

## 二、`setSpanSteps`（`0x100498c0`）——三趟遍历，两个数组整个 collar 共一份

先按 `nSpans = (D2_Profile+56 − D2_Profile+52) >> 3` 开两个长 `nSpans+1` 的
`FL_vector<int>`：**`nSteps` 初值填 8**（整圆最少 8 段），**`pair` 初值填 −1**。
然后对 `linkedProfiles()`（两端外环 + 全部孔环的集合）走三趟：

```text
遍 1（配对，跨端共享）
  for p in linkedProfiles():
      if p.state == 2: p->虚表[+32]()          // 延迟校验
      if p.state != 4: continue
      for i in 1..=nSpans:
          if pair[i] < 0:
              j = GM_Profile::pairedSpan(p, i)
              if j > 0: pair[i] = j; pair[j] = i

遍 2（段数，跨端取大）
  for p in linkedProfiles():
      tol_p = *(double*)(p + 0x18)             // 每个轮廓自己烤进去的 arctol_
      for i in 1..=nSpans:
          r_own  = D2_Span::getRadius(D2_Profile::getSpan(p + 40, i))
          r_pair = pair[i] > 0 ? D2_Span::getRadius(getSpan(p + 40, pair[i])) : 0.0
          n = d2_numberOfSegmentsForCircle(fmax(r_pair, r_own), tol_p)
          nSteps[i] = max(nSteps[i], n)        // 只增不减

遍 3（写回）
  for p in linkedProfiles():
      p+0x50 ← nSteps 副本                     // 轮廓自己的步数向量
      p+0x60 ← 1                               // 「步数已被外部设定」标志
```

三件转述里没有的事：

1. **`pair` 表也是跨端共享的**，不只是段数。底面找到的配对，顶面直接用（先到先得）。
2. **容差是逐轮廓的**（`p+0x18`），两端 tol 不同时由「取大」吸收，不是取某一个。
3. **`nSteps` 的初值是 8**，不是 0——直段（半径 0）也会拿到 8，只是 `getNFacetsRoundProfile`
   把直段按 1 计，落不到面片上。

`setNSteps(FL_vector<int>&)`（`0x1008f130`）做的正是遍 3 那件事（拷贝 + 置 `+0x60`），
`setSpanSteps` 把它内联了。

## 三、`calcFacetsWithoutSurfaces`（`0x10048500`）与「单调取大」的真正用途

调用顺序：

```text
base = this+0x14 → [+8]  → GM_Profile*
top  = this+0x14 → [+12] → GM_Profile*
setSpanSteps()
polygonForFacet(base, &basePoly, &baseFlags)
polygonForFacet(top,  &topPoly,  &topFlags)
if (D2_Profile::area(base) < 0)  两条折线与两个标志向量一起 reverse
nBase = basePoly.size - 1 ;  nTop = topPoly.size - 1
GM_Facets::reSize(nBase + 1, 2 * nBase, nBase + nTop)
top.state == 4 ? formTopFacet(topPoly, …, height, …) : formTopSides(…)
formBaseFacet(basePoly, …)
<侧壁归并走查>
```

**绕向由底面轮廓的有符号面积裁决**，负则两端一起翻——顶面不单独判。

`polygonForFacet`（`0x1008ed80`）的分支值得单记，因为它决定了 collar 的统一值会不会被冲掉：

```c
tol = this+0x18 ;  cachedTol = this+0x48
if (cachedTol == tol) {
    if (输出多边形为空) {
        if (!this[0x60]) setNSteps(tol);      // 标志命中就跳过
        return getPolygonForFacet(out);
    }
} else {
    if (this[0x60]) { this[+4] = 2; this[0x60] = 0; this+0x48 = <哨兵> }
    setNSteps(tol);                            // ← 标志被自己清掉了，照样重算
    total = getNFacetsRoundProfile();
    if (total > 1000) {
        printLimitFacetWarning(total);
        this[22] = this[21];                   // 清空步数数组
        tol' = tol * ((total − nSpans)² / (1000 − nSpans)²);
        setNSteps(tol');
    }
    getPolygonForFacet(out);
    this+0x48 = 实际用的 tol;
}
```

也就是说 **`+0x60` 标志在冷路径上根本挡不住重算**。collar 的跨端统一值之所以还在，
靠的是 `setNSteps(double)` 那条「只增不减」——单个轮廓自算的值不可能大于跨端最大值，
重算一遍等于没变。**「单调取大」不是实现细节，它就是 collar 强制两端同段数的机制本身。**

**顺带坐实一个隐患**：1000 封顶那支会 `this[22] = this[21]` 把步数数组**清空**再整体重标定，
而它是**逐轮廓**判的。一端超限、另一端不超时，collar 的统一就碎了，两端点数随之分家
（validate 比的是轮廓跨度数，不是折线点数，拦不住这种）。本仓两端同截面，暂时够不着，
但复刻时不要把「统一」当成不变量。

## 四、三个 `form*`

- **`formBaseFacet`（`0x10048d60`）**：逐点 `addVertex(x, y, **0.0**)`；相邻点
  `addEdge(v[i-1], v[i], label, **类型 2**)`；side 码写 `2*e + 1`（奇数 = 反向），
  且**数组倒序填**；收尾一条闭合边后 `addFacet(sides, label, 5)`——
  **底盖是一个 n 边形面，不是三角扇**。
- **`formTopFacet`（`0x10048f20`）**：与上逐字同构，只差两处——`addVertex(x, y, **height**)`，
  side 码写 `2*e`（偶数 = 正向）。两个盖绕向相反，闭合实体成立。
  **所以直纹体是 z = 0 → z = height，不是 ±h/2**：与 box / cylinder / snout / pyramid
  那批「上下各摊一半」的约定不同族，抄的时候别顺手居中。
- **`formTopSides`（`0x100490d0`）**：**一个面都不建**。逐点拿新点与已建顶点倒着比，
  三个坐标（含 z = height）**精确相等、无 epsilon** 就复用那个顶点与边并回退计数，
  否则新建顶点 + `addEdge(…, 类型 2)`。这是顶面轮廓没过校验（`state ≠ 4`）时的退路，
  走的是「出去再折回来」的片体形态。

## 五、侧壁是双指针归并，且第二出参一参两用

侧壁不是「逐点连线」：两个环各拿一个倒计数，值来自 `polygonForFacet` 的**第二出参**
（`FL_vector<int>`，逐顶点的计数，负号 = 硬边，即 §7.9.2 第 4 条那个出参；下一节把它
读了）。谁先归零谁前进：只一边前进出三角形（三条 side），两边都前进出四边形
（四条 side），每步 `addFacet(sides, label, 5)`。竖边的 `GM_EdgeType` 直接由符号定：
两端都是负 → 2，否则 → 1。

**这把 T040b 提了一档**：`getPolygonForFacet` 的第二出参不只是「曲面法向该怎么分组」的
权威，它的**绝对值是直纹面归并走查的推进量**。一个出参背着两件事，只实现半个不行。

## 六、顺着上一节把 `getPolygonForFacet` 与 `leadsSmoothlyTo` 也读了（T040b 的权威）

### `GM_Profile::getPolygonForFacet(D2_Polygon&, FL_vector<int>&)`（3.1 libgm `0x1008F8B0`）

```text
flags.clear()                                  // *(out+8) = *(out+4)，长度归零
span = getSpan(1)
for i in 1..=nSpans:
    if span.p0 != span.p1:                     // 非退化段（精确比较，无 epsilon）
        poly.addPolyLine(D2_Span::getApproxPolyLineInSteps(span, nSteps[i]))
        flags 扩到 poly 的点数，新槽初值 0
    ++flags[poly.len − 1]                      // 「又有一条 span 在这个点收尾」
    if i < nSpans:
        next = getSpan(i + 1)
        if next 非退化:
            if poly.len == 1:  span = next     // 还没铺出点，直接换段
            else if !D2_Span::leadsSmoothlyTo(span, next):
                flags[poly.len − 1] = −flags[poly.len − 1]     // ← 硬边取负
        span = next
// 闭合处（首尾 span 相接）：
if poly 非空 且 (poly.front() != poly.back() 或 !leadsSmoothlyTo(lastSpan, firstSpan)):
    flags[末] = −flags[末] ;  flags[0] = −flags[0]              // 两端一起取负
```

两件此前描述得不够准的事：

1. **`|flags[k]|` 不是「游程长度」，是「有几条 span 在第 k 个顶点收尾」**，正常恒为 1；
   只有连续退化段（`p0 == p1`）会往同一个下标上累加。collar 的归并走查拿它当推进量，
   所以两端退化段分布不同的时候，走查就会在一侧停一拍、出三角形而不是四边形。
2. **闭合处一次负两个**（末点与首点），且触发条件是**或**：折线首尾点不重合**或**
   末段不平滑接回首段。前者是「这条轮廓压根没闭合」，也按硬边处理。

### `D2_Span::leadsSmoothlyTo`（**libgeom** 3.1 `0x10029B50`，libgm 只是导入方）

```c
getLastTangent(this, t0);
getFirstTangent(other, t1);
return fabs(1.0 - (t0.x * t1.x + t0.y * t1.y)) <= 1e-6;
```

**判据是 `1 − 点积 ≤ 1e-6`，换成夹角约 0.081°。** 这是本条线上遇到的**第三个**相切判据，
三个互不相同、也不能互相顶替：

| 判据 | 阈值 | 用途 |
|---|---|---|
| `D2_Span::leadsSmoothlyTo` | `1 − dot ≤ 1e-6`（≈0.081°） | 轮廓顶点是不是硬边（本条） |
| `isTangentDiscontinuity` | 固定 22.5° | 布尔交线边定型（§6.8.3） |
| `isSharp` | `K ≈ 0.8182`（≈48.3°） | 归一化里的折角判据（§6.6） |

切线本身来自 `D2_Span::getFirstTangent` / `getLastTangent`（libgeom `0x100296F0` /
`0x10029930`，`D2_Span` 布局 `+0 p0 / +2 p1 / +4 bulge / +5 centre / +7 signedRadius`）：

- `bulge == 0.0`（**精确等于**）→ 弦向单位向量；零长度段回 `(0, 0)`。
- `|bulge| ≥ 3.06e-5` → 先按需 `calcCentreAndRadius`，`r = 端点 − 圆心`，
  切向 = `(−r.y, r.x) / R`，其中 `R` 取带符号半径、`bulge ≤ 0` 时再取反。模长恒为 1。
- **`0 < |bulge| < 3.06e-5` 走一条退化分支**：圆心取弦中点、`R` 取 −1，于是切向是
  `(±半弦向量)` 旋转 90° 后的**非单位**向量。这样的段与邻段的点积几乎不可能落进
  `1e-6`，**实际效果是「极小但非零的 bulge 必定被判成硬边」**。照抄时别把它「顺手
  归一化」——归一化会把这个行为改掉。

T040b 要的就是这一套：`|flags[k]|` 给推进量、符号给硬边、硬边判据是这个 1e-6 的点积。

## 对本仓的落地项

1. **口径**（T056，**当日已落地**）：`sweep_mesh::flatten_loop` 原先走
   `span_polyline_by_tol`（挤出的整圆角度格子），而 §7.9.2 点名 `setNSteps` 那套只服务
   `GM_Revolution` 与 `GM_Collar`——这两个东西在本仓都落在 `sweep_mesh.rs`。T040 在
   `manifold_tessellate::tessellate_revolution` 上修掉的错，扫掠这条路上原样留着。
   现改为三个入口（`profile_loops` / `profile_loops_revolved` / `profile_loops_ruled`）
   对应 libgm 的三个类，跨环取大只给放样支。经过与门见 tasks.md 的 T056。
2. **前置**（T022 门 a）：两端跨度数不等 → libgm −61 硬拒；本仓应 `bail!`，不得默认成立。
3. **跨端统一**（T056 的一部分）：本仓 `libgm_discretise::paired_span` 只在本环内找配对，
   collar 要的是「两端外环 + 全部孔环」一份共享的 `pair` 表与 `nSteps` 表，初值 8、逐 span 取大。
4. **摆位**（T022 门 c）：z = 0 → z = height。本仓放样支
   （`btm = get_face_mat4(true)`、`top = translate(Z·height) · get_face_mat4(false)`）
   恰好也是 0..height，**这一条现状是对的**，加测试钉住即可。
