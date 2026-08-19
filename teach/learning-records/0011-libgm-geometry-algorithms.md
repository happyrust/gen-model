# 0011 — libgm.dll 几何算法全解：从 `gm_Create*` 到三角面片

- **日期**：2026-08-16
- **背景**：`0009` 定位到"真正的实体建模在 `libgm`"后就停在了调用面。本次把 `libgm.dll` 内部**逐个图元的三角化算法**、**容差体系**、**CSG 布尔核**、**法线平滑**、**出料索引流编码**、**消隐空间细分**全部拆开。
- **样本**：`D:\AVEVA\Everything3D2.10\libgm.dll`（780 576 B，2022-09-20），`GM_version()` 返回 **`8.4.9.0`**。配套 `D:\AVEVA\Everything3D2.10\libgeom.dll`（367 904 B，新建 IDB）。
- **工具**：`ida-bridge`，两个 idalib 实例（libgm / libgeom）。
- **相关**：`0009`（批量流水线）、`teach/MISSION.md` 课 03 遗留第 3 条。

---

## 一、这个库是什么

`libgm` = **GML（Geometric Modelling Library）**，AVEVA 自研的**面片式（facet-based）实体建模器**。不是 B-rep 精确核（不是 ACIS/Parasolid 那一类），它的唯一真理是**多边形面片集合**，解析曲面只作为"附加信息"挂在面片上。

导出 **2677** 个符号，几乎整个 C++ API 都带 MSVC mangled 名，因此类名、成员名、参数类型全部可读——这是本次能拆得这么细的前提。

### 依赖面

| 模块 | 导入数 | 作用 |
|---|---|---|
| `libgeom` | **170** | `D2_*`/`D3_*` 数学（点/向量/矩阵/变换/限界/多边形/PolySet 布尔），以及**分段数公式** `d2_numberOfSegmentsForCircle` |
| `MSVCP100` / `MSVCR100` | 57 / 53 | VS2010 运行库 |
| `KERNEL32` | 17 | — |
| `libfl` | 16 | `FL_NodeHeap`（自定义节点堆）、`FL_Monitor`（每函数计时探针）、`FL_SystemClock` |
| **`GLU32`** | **10** | `gluNewTess`/`gluTessCallback`/… —— **只被 `GM_ExtrusionGroup::calcFacets` 用**（见 §5.12） |

### 源文件清单（从 `gm_reportInternalFault` 的 `__FILE__` 字面量还原）

`AM_Body.cxx` `AM_CoEdge.cxx` `AM_Face.cxx` `AM_Loop.cxx` `AM_SGL.cxx`
`GM_3DGeomUserFacets.cxx` `GM_3DItem.cxx` `GM_CC.cxx` `GM_CompFacets.cxx`
`GM_ComplexCombination.cxx` `GM_ComplexUserFacets.cxx` `GM_EDish.cxx` `GM_Edge.cxx`
`GM_Facets.cxx` `GM_IdMap.cxx` **`GM_Intersect.cxx`** `GM_Item.cxx` `GM_MeshSurface.cxx`
`GM_PolygonMesh.cxx` `GM_Polyhedron.cxx` `GM_PositionedItem.cxx` `GM_Profile.cxx`
**`GM_ProfileTessellator.cxx`** `GM_Section.cxx` **`GM_SetOp.cxx`** `GM_SlopeEndCyl.cxx`
`GM_SolidCombination.cxx` `GM_SolidUserFacets.cxx` `GM_SuperFacet.cxx` `HL_Cell.cxx`

### 类族速览（mangled 名里共 141 个类前缀，含 STL 与编译器生成项；自有类 ~120）

| 前缀 | 含义 | 代表 |
|---|---|---|
| `GM_*` | 建模器主体 | `GM_Item` 树、`GM_Facets` 面片集、各图元 |
| `AM_*` | **A**nalytic **M**odel = 面片集导出的 B-rep 视图 | `AM_Body/Face/Loop/CoEdge/Edge/Vertex/Surface/SGL` |
| `D2_*` `D3_*` | libgeom 的 2D/3D 数学 | `D3_Transform` `D2_PolySet` `D3_LimitsArray` |
| `HL_*` | **H**idden **L**ine 消隐（DRAFT 出图） | `HL_Picture` `HL_Cell` `HL_SceneElement` |
| `FL_*` | libfl 基础设施 | `FL_NodeHeap` `FL_Monitor` |

---

## 二、对象模型：句柄 + 单继承树

### 2.1 句柄不是指针

所有公共 API 收发的都是 `unsigned int` **id**，不是指针。

```c
// GM_Item::GM_Item(double tol)  @ 0x10009160
++g_nextId;                       // dword_100B4F60，全局单调计数器
GM_User::idMap().add(this);       // GM_IdMap，红黑树 id -> GM_UserObject*
this->id_ = g_nextId;             // +0x08
this->tolerance_ = max(tol, 1e-6);// +0x18
```

- **id 单调递增、永不复用**。`GM_IdMap::get(id)` 走 `std::map`，找不到返回 0。
- 每个 API 入口都先过 `GM_Check<T>::checkIdAndWarn(id, "GMXXXX/gm_Xxx")`：查表 + 动态类型检查 + 失败时 `gm_message` 告警并返回 0。**没有静默失效**——这一点值得我们抄。

### 2.2 `GM_Item` 内存布局（32 位）

| 偏移 | 内容 |
|---|---|
| `+0x00` | vftable |
| `+0x04` | `= 2`（`GM_UserObject` 状态位/引用标志） |
| `+0x08` | **id_** |
| `+0x0C..0x14` | owner / member 链表头（`GM_Instance` 双向链） |
| `+0x18` | **`tolerance_`**（double，下限 `1e-6`） |
| `+0x20` | **`label_`**（int，默认 `GM_User::label_ = -1`） |
| `+0x28` 起 | 派生类的 double 参数区 |

### 2.3 `GM_Types::types` 全枚举

从 48 个 `X::staticDesc()` 的返回常量逐一还原（`gm_QueryType(id)` 返回它）：

| 值 | 类 | 值 | 类 | 值 | 类 |
|---|---|---|---|---|---|
| 1 | `GM_Item` | 20 | `GM_Extrusion` | 34 | `GM_SolidCombination` |
| 2 | `HL_Picture` | 21 | `GM_ExtrusionGroup` | 35 | `GM_SolidNormalised` |
| 3 | `GM_3DItem` | 22 | `GM_RectTorus` | 36 | `GM_SolidSection` |
| 4 | `GM_Aggregate` | 23 | `GM_CircTorus` | 37 | `GM_SolidUserFacets` |
| 5 | `GM_AggregateCombination` | 24 | `GM_SDish` | 38 | `GM_3DGeom` |
| 6 | `GM_AggregateNormalised` | 25 | `GM_EDish` | 39 | `GM_3DGeomPrimitive` |
| 7 | `GM_AggregateSection` | 26 | `GM_Sphere` | 40 | `GM_Straight` |
| 8 | `GM_AggregateUserFacets` | 27 | `GM_Null` | 41 | `GM_Bezier` |
| 9 | `GM_Complex` | 28 | `GM_Pyramid` | 42 | `GM_Arc` |
| 10 | `GM_ComplexCombination` | 29 | `GM_Snout` | 43 | `GM_Mark`（由 `gm_QueryItem` case 43 反推） |
| 11 | `GM_ComplexNormalised` | 30 | `GM_Cylinder` | 44 | `GM_MeshSurface` |
| 12 | `GM_ComplexSection` | 31 | `GM_Block` | 45 | `GM_UserPrimitive` |
| 13 | `GM_ComplexUserFacets` | 32 | `GM_SlopeEndCyl` | 46 | `GM_3DGeomNormalised` |
| 14 | `GM_Solid` | 33 | `GM_Polyhedron` | 47 | `GM_3DGeomSection` |
| 16 | `GM_SolidPrimitive` | 17 | `GM_Collar` | 48 | `GM_3DGeomUserFacets` |
| 18 | `GM_SweptSolid` | 19 | `GM_Revolution` | 49 | `GM_CutSurface` |
| | | | | 50 | `GM_Profile` |

**15 未出现**（无对应 `staticDesc`），未解。

### 2.4 四种"包装器"的 4×4 矩阵

`{Solid, Complex, Aggregate, 3DGeom} × {Combination, Section, Normalised, UserFacets}` —— 16 个类，是同一套语义在四种"内容类型"上的复制：

- **Combination**：CSG 组合节点（并/交/差）。
- **Section**：被切割（`gm_CreateSection`）。
- **Normalised**：面片归一化包装（合并共面、消退化）。
- **UserFacets**：**"已经算完面片、可以往外交付"的终态包装**。`gm_QueryFacetData` 只接受 37 / 13 / 48 三种，即三个 `*UserFacets`。所以 Core3D 的正确用法是先 `gm_CreateFacetStructure(id)` 再 `gm_QueryFacetData(newId, …)`。

---

## 三、公共 API 面（618 个自由函数）

按前缀分组，全部 `__cdecl`：

| 组 | 代表 | 说明 |
|---|---|---|
| **建图元** | `gm_CreateBox/Cylinder/Snout/Pyramid/SphericalDish/EllipticalDish/CircularTorus/RectangularTorus/Sphere/SlopeEndedCylinder/Extrusion/ExtrusionGroup/Revolution/RuledSolid/Polyhedron/MeshSurface/UserPrimitive/Null` | 返回 id |
| **建曲线/轮廓** | `gm_CreateStraight/Arc/Bezier/Mark/Profile/CutSurface`，`gm_AddSpan/AddEndSpan/AddCurve/AddCutProfilePoint` | 2D 轮廓 & 3D 曲线 |
| **CSG** | `gm_CreateCombination(op)` `gm_AddMember` `gm_CreateSection(id,op,cut)` `gm_CreateClippedTree` `gm_CreateExpandedTree` `gm_CreateSolidTree` `gm_CompressTree` `gm_CreateNormalisedItem` | 树装配 |
| **变换** | `gm_CreateTransform` `gm_SetTransform` `gm_ShiftTransform` `gm_RotateTransform` `gm_SetIdentityTransform` `gm_GetTransform` | `D3_Transform` |
| **容差/属性** | `gm_SetDefaultFacetTolerance` `gm_SetFacetTolerance(id,t)` `gm_SetFacetToleranceForTree` `gm_SetDefaultNormalisationTolerance` `gm_SetDefaultTangentTolerance` `gm_SetResolutionTolerance` `gm_SetLabel` `gm_SetLabelMap` `gm_SetTransparency` | 见 §4 |
| **出面片** | `gm_CreateFacetStructure(id)` / `…WithSurfaces(id)` → `gm_QueryFacetDataSize` / `gm_QueryFacetData` / `gm_QueryEdgeData(Size)` | 见 §7 |
| **查询** | `gm_QueryType/Item/Limits/Mass/Ray/XRay/Clash/Close/CloseLimits/Equals/IfHasHoles/NumberOfMembers/Owner/Member/MemoryUse*` | |
| **遍历** | `gm_CreateIterator` `gm_IteratorNextItem` `gm_IteratorGetItem` `gm_IteratorGetRelativeTransform` `gm_IteratorGetSense` `gm_IteratorNoDescend` `gm_IteratorReset` | 树游标 |
| **消隐出图** | `gm_CreatePicture` + 14 个 `gm_Picture*`（`Style/Perspective/Gapping/ArcInfo/FacetLines/CellSplitFactor/SelectHiddenFace/SelectSuppressedLines/Draw`…） | `HL_*` |
| **诊断** | `gm_OutputTree/OutputObject/DebugOutputFacetData/DebugSetLevel/FileListing/Monitoring*/ValidateObject/ValidateTree` | |

### 3.1 GML 六字母助记符 —— 与 PDMS 侧的接缝

每个 `gm_Create*` 在 debug 日志里都会写一行 GML 命令，助记符正是 PDMS 传统的 6 字母例程名。这是 `libgm` 与 PDMS 目录图元之间**唯一确凿的命名接缝**：

| GML | C++ API | 参数（按源码顺序） |
|---|---|---|
| `gmcbox` | `gm_CreateBox` | `xLength, yLength, zLength`（**全长**，体心在原点） |
| `gmccyl` | `gm_CreateCylinder` | `radius, height` |
| `gmcsnt` | `gm_CreateSnout` | `rBottom, rTop, height, xOffset, yOffset` |
| `gmcpyr` | `gm_CreatePyramid` | `xBottom, yBottom, xTop, yTop, height, xShift, yShift` |
| `gmcsds` | `gm_CreateSphericalDish` | `radius, height` |
| `gmceds` | `gm_CreateEllipticalDish` | `baseRadius, height, knuckleRadius` |
| `gmccto` | `gm_CreateCircularTorus` | `rInside, rOutside, startAngle°, finishAngle°` |
| `gmcrto` | `gm_CreateRectangularTorus` | `rInner, rOuter, height, startAngle°, finishAngle°` |
| `gmcslc` | `gm_CreateSlopeEndedCylinder` | `radius, height, xBase°, yBase°, xTop°, yTop°` |
| `gmcsph` | `gm_CreateSphere` | `radius` |
| `gmcrev` | `gm_CreateRevolution` | `startAngle°, finishAngle°, origin(D2), axisAngle°, profileId` |
| `gmcxtr` | `gm_CreateExtrusion` | `profileId, height` |
| `gmcarc` `gmcbez` `gmcbody` … | — | 曲线族 |

**注意 `gmcslc` 的实参顺序被打乱**：`gm_CreateSlopeEndedCylinder(a1..a6)` @ 0x1001cde0 落到字段是 `+5=a1(radius) +6=a2(height) +7=a5 +8=a6 +9=a3 +10=a4`。哪两个是底、哪两个是顶，靠 libgm 单侧读不出来；由上游 `CSG_BasicSLC::getPrimGeom` 定死（§11.3）：`a3/a4 = XBSH/YBSH`（底），`a5/a6 = XTSH/YTSH`（顶）。因此**存储上 `+7/+8` 是顶端两角、`+9/+10` 是底端两角，与形参顺序正好对调**。抄参数时要区分"调 API"（用形参顺序：底在前）与"读字段"（用偏移：顶在前）。

`gmccto` 的头两个参数是**内外半径**不是"中心线半径 + 管半径"——由 `CSG_BasicCTO::getPrimGeom` 直传 `ATT_RINS` / `ATT_ROUT` 坐实，并与 §3.2 里 `gm_QueryItem` 反向输出 `(rIn+rOut)/2` / `(rOut−rIn)/2` 的换算自洽。

### 3.2 `gm_QueryItem` 的参数向量顺序

`gm_QueryItem(id, &type, &vector<double>)` 是反向读参数的权威。它是一个 switch，**输出顺序与创建顺序不总是一致**：

| type | 参数向量 |
|---|---|
| 30 `Cylinder` | `[radius, height]` |
| 31 `Block` / 25 `EDish` | `[+5, +6, +7]` |
| 24 `SDish` | `[radius, height, 0]` |
| 29 `Snout` | `[rTop, rBottom, height, xOff, yOff]` ← **前两个与创建时相反** |
| 23 `CircTorus` | `[(rIn+rOut)/2, (rOut−rIn)/2, finishAng, startAng]` ← **换算成中心半径+管半径** |
| 22 `RectTorus` | `[+5, +6, +7, +9, +8]` |
| 28 `Pyramid` | `[xTop, yTop, xBottom, yBottom, height, xShift, yShift]` |
| 32 `SlopeEndCyl` | `[r, r, h, 0, 0, +7, +8, +9, +10]`（9 个） |
| 17/26 `Collar`/`Sphere` | `[+5]` |
| 20/21 `Extrusion`/`Group` | `[height]` |
| 19 `Revolution` | `[+9, +10, +11, +8, +7]` |
| 40 `Straight` | `[P0.xyz, P1.xyz]` |
| 41 `Bezier` | `[P0.xyz, P1.xyz, PC.xyz, weight]` |
| 42 `Arc` | `[+5, +6, +9, +7, +8]` |
| 43 `Mark` | `[P.xyz]` |

---

## 四、容差体系（★核心）

### 4.1 四个全局容差

`GM_User` 的静态字段，DLL 映像里的编译期初值：

| 字段 | 初值 | setter | 用途 |
|---|---|---|---|
| `arctol_` | **0.1** | `gm_SetDefaultFacetTolerance` | **弦高容差**，新建图元的默认 `GM_Item::tolerance_` |
| `normtol_` | **1e-6** | `gm_SetDefaultNormalisationTolerance` | 顶点合并 / 归一化容差 |
| `tangtol_` | **5.0** | `gm_SetDefaultTangentTolerance` | 相切判定（**度**） |
| `restol_` | **0.1** | `gm_SetResolutionTolerance` | 分辨率容差 |
| `label_` | −1 | `gm_SetDefaultLabel` | 新建图元默认 label |
| `maxPrimitiveLength()` | **1e7** | 硬编码 | 图元尺寸上限（`validate` 用） |
| `maxFacetsForProfile()` | **1000** | 硬编码 | 轮廓面片数上限 |

`gm_CreateBox` / `gm_CreatePyramid` / `gm_CreateCombination` 传的是**字面量 0.1**（平面体无曲率，容差无意义）；其余曲面图元传 `GM_User::arctol_`。
`gm_SetFacetTolerance(id, t)` 把 `t` 钳到 `≥1e-6` 后写 `GM_Item+0x18`。

### 4.2 分段数公式（在 libgeom，不在 libgm）

```c
// libgeom!d2_numberOfSegmentsForCircle(radius, tol)  @ libgeom 0x1001d550
int d2_numberOfSegmentsForCircle(double r, double tol)
{
    if (r <= 0.0) return 1;
    double t   = fabs(tol / r);
    double c   = 1.0 - t;  if (c <= 0.0) c = 0.0;
    double deg = degrees(2.0 * acos(c));      // 单段圆心角
    if (deg <= 0.0 || deg > 45.0) deg = 45.0; // 下限 8 段
    int n = (int)ceil(360.0 / deg);
    return 4 * ((n + 3) / 4);                 // 向上取整到 4 的倍数
}
```

这就是经典的**弦高（sagitta）判据**：弦高 `s = r(1 − cos(θ/2))`，令 `s = tol` 解出 `θ = 2·acos(1 − tol/r)`。

三条工程化处理值得注意：

1. **单段角上限 45°** ⇒ 最少 8 段。半径小于 `tol / (1 − cos22.5°) ≈ 1.31 × tol` 时永远是 8 段。
2. **向上取整到 4 的倍数** ⇒ 多边形顶点必落在 ±X / ±Y 轴上。这保证了包围盒精确、以及同半径的不同图元顶点角度一致（对后续布尔的顶点焊接极其重要）。
3. 上限**不在这里**，在各图元的 `calcFacets` 里：`if (n > 1000)` → `gm_message(1001, "GM_Xxx - facet tolerance too small for radius, adjusted")` 然后 `n = 1000`。**这是一条会打日志的静默降级**，Core3D 侧看不到返回码。

近似式：`n ≈ π·√(r / (2·tol))`。`tol = 0.1 mm` 时的实测值：

| 半径 mm | 0.5 | 1.3 | 2 | 5 | 10 | 25 | 50 | 100 | 250 | 500 | 1000 | 5000 | 20300 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 段数 | 8 | 8 | 12 | 16 | 24 | **36** | 52 | 72 | 112 | 160 | 224 | 500 | **1004→截 1000** |

> `r = 25 mm`（DN50 管）恰好是 36 段。我们 `mesh_primitives.rs` 里那个 `DEFAULT_CIRCULAR_SEGMENTS = 36` 只在这一个半径上与 AVEVA 一致，见 §10。

### 4.3 部分回转的分段

```c
// d2_numberOfSegmentsForPartRev(r, tol, &startDeg, &endDeg, &isFull)
把 end 规范化到 (start, start+360]
nFull = d2_numberOfSegmentsForCircle(r, tol)
if |sweep| ≤ 1e-6 或 |sweep − 360| ≤ 1e-6:
      isFull = true;  start = 0; end = 360;  return nFull
else: isFull = false; return max(2, ceil(sweep / (360 / nFull)))
```

即**部分弧沿用整圆的角步长**，不重新分配——同一半径的整圆与弧段顶点角度对齐。

### 4.4 轮廓（`GM_Profile`）的分段

- `setNSteps(tol)`：逐 span 取 `max(半径, 配对 span 半径)` 算段数，与已有值取 `max`。**配对 span（`pairedSpan`，例如环形轮廓的内外同心弧）强制同段数**。
- `getNFacetsRoundProfile()`：`Σ |Δα| · nSteps[j] / 2π`（直线段记 1）。
- `adjustTolerance(&tol, n)`：`tol *= ((n − nSpans) / (1000 − nSpans))²` —— 面片数超 `maxFacetsForProfile()=1000` 时用**二次退避**把容差放大，回落到上限附近。
- 弧/直线判据：`|bulge| < 3.06e-5` 视为直线（`D2_Span::getApproxPolyLine` 与 `getNFacetsRoundProfile` 共用这个常数）。
- `D2_Span::getApproxPolyLineInSteps(n)` 的中间点取在**全局角度栅格** `k·2π/n` 上（只保留落在 `(α0, α1)` 内的），不是把弧本身均分。

---

## 五、逐图元三角化算法

统一约定：`GM_Facets(nFacets, nEdges, nVertices)` 三个参数是**预留容量**（`Block` 传 `(6,12,8)` 可验证）。`GM_FacetReqs` 只有两个 bool：`surfacesWanted` / `circlesWanted`；任一为真就 `ensureHasSurfaces()`。

### 5.1 `GM_Block`（`gmcbox`）— `calcFacets` @ 0x10013ca0

纯解析，无容差参与。半长 `hx=x/2, hy=y/2, hz=z/2`，**体心在原点**。8 顶点按
`(−hx,−hy,−hz) (−hx,+hy,−hz) (+hx,+hy,−hz) (+hx,−hy,−hz)` 再同序 `+hz`；
12 条边全部 `GM_EdgeType = 2`（硬边）；6 个面用 `addFacetAndFlatSurface`。

### 5.2 `GM_Cylinder`（`gmccyl`）— `calcFacets` @ 0x1002f830

```
n   = d2_numberOfSegmentsForCircle(r, tol)      // >1000 → 告警并截断
dA  = 2π / n
for i in 0..n-1:
    a = i·dA;  x = r·cos a;  y = r·sin a
    addVertex(x, y, −h/2)      // 偶数下标 = 底
    addVertex(x, y, +h/2)      // 奇数下标 = 顶
    底圈边 addEdge(2i−2, 2i, label, type=2)  ← 逆序写入数组（底盖法线朝外）
底盖 addFacetAndFlatSurface(底圈)
顶圈边 addEdge(2i−1, 2i+1, label, type=2)
顶盖 addFacetAndFlatSurface(顶圈)
若要曲面: addSurface(type=2 /*柱面*/, point=(0,0,0), vector=(0,0,r), label)
for i in 0..n-1:
    竖边 addEdge(2i, 2i+1, label, type=1)   ← type 1 = 平滑
    侧面四边形 addFacet([底边ᵒᵖ, 竖边ᵢ, 顶边ᵒᵖ, 竖边ᵢ₋₁ᵒᵖ], surfaceIdx, state=5)
```

- 顶点数 `2n`，边数 `3n`，面数 `n+2`。
- 圆柱面 `addSurface` 的编码很省：**方向向量 = 轴向单位向量 × 半径**，`(0,0,r)` 同时表达轴和半径。

### 5.3 `GM_Snout`（`gmcsnt`）— `calcFacetsWithoutSurfaces` @ 0x10066510

字段：`+5 = rBottom`，`+6 = rTop`，`+7 = height`，`+8 = xOffset`，`+9 = yOffset`。

```
h2 = height/2;  ox = xOffset·0.5;  oy = yOffset·0.5
n  = d2_numberOfSegmentsForCircle(max(rBottom, rTop), tol)
底顶点 = (cos a·rBottom − ox,  sin a·rBottom − oy,  −h2)
顶顶点 = (cos a·rTop    + ox,  sin a·rTop    + oy,  +h2)
```

**偏移是对半劈开的**：底心在 `(−ox,−oy)`，顶心在 `(+ox,+oy)`，两者相距正好 `(xOffset, yOffset)`。
另有两条退化分支：`rTop ≈ 0` 或 `rBottom ≈ 0`（`|r| ≤ 1e-6`）时那一端塌成单个尖点，生成 n 个三角形而不是四边形；两端都为 0 直接返回空面片集。

### 5.4 `GM_SlopeEndCyl`（`gmcslc`）— @ 0x10065da0

四个角度先转弧度取正切：`t = tan(deg · 0.0174532925199433)`。

```
若任一角 ≥ 90° → 返回空 GM_Facets
n = getNCoords() = d2_numberOfSegmentsForCircle(radius, tol)   // 同样 >1000 告警
判据: height − sqrt((tx1−tx2)² + (ty1−ty2)²)·radius ≤ 0 ?
   是 → 两端斜面相交，走 sub_10064140(op=2 /*INTERSECTION*/, …) 做一次真布尔
   否 → 走 sub_10065430(...) 的解析闭式：直接把两端顶点抬到斜面上
```

本次拆到的图元里，**只有它会在内部回调 CSG 核**：两斜面相交时，斜端圆柱退化成"圆柱 ∩ 两半空间"，走真布尔；不相交则用闭式把两端顶点直接抬到斜面上。

### 5.5 `GM_Sphere`（`gmcsph`）— @ 0x10068d30

标准 UV 球：`n` 条经线 × `n/2` 条纬带。

```
n = d2_numberOfSegmentsForCircle(r, tol)
GM_Facets(n²/2, n(n−1), n(n/2−1)+2)
两极 addVertex(0,0,±r)
for i in 0..n-1:  θ = i·2π/n
  for j in 1..n/2-1: φ = j·π/(n/2)
      addVertex(r·sinφ·cosθ, r·sinφ·sinθ, r·cosφ)
所有边 type = 1（全平滑）；极点处收成三角形，其余四边形
```

### 5.6 `GM_SDish` 球缺（`gmcsds`）— @ 0x10062170

```
n  = d2_numberOfSegmentsForCircle(radiusOfSphere(), tol)
半张角 φ: t = height/R
     if t > 1e-6:  φ = acos(1 − t)
     else:         φ = sqrt(2t)         ← 小角度数值稳定分支
```

`acos(1−t) ≈ √(2t)` 是同一个函数在 `t→0` 的展开——`t ≤ 1e-6` 时 `1−t` 的浮点抵消会毁掉 `acos` 的精度，所以换成解析近似。`GM_EDish` 里也有同一段代码。
`height ≤ 0` → 空面片集。

### 5.7 `GM_EDish` 椭圆封头（`gmceds`）— @ 0x10030ca0

字段：`+5 = baseRadius`，`+6 = height`，`+7 = knuckleRadius`。这是**两段圆弧（球冠 hub + 过渡 knuckle）**回转体：

```c
// knuckleRadiusToUse()  @ 0x10032080
rk0 = height;  d = baseRadius − height
r   = rk0 / (d / sqrt(rk0² + baseRadius²) + 1.0)
if |d| ≤ 1e-6            return height          // 半球
if height >= baseRadius  return (baseRadius − knuckle > 1e-6) ? r : knuckle
if knuckle − height < −1e-6 return knuckle
return r
// radiusOfHub()  @ 0x10032110
= (height² + baseRadius² − 2·rk) / (2·(baseRadius − rk))
```

分段：`nHub = numberOfSegmentsForPartRev(rHub, tol, 0°, φ°)`，`nKnuckle = …(rk, tol, φ°, 90°)`；上限检查分三条独立告警（"base radius" / "knuckle radius" / "hub radius"）。

上面那条 `r = rk0 / (d / sqrt(rk0² + baseRadius²) + 1.0)` 不只是 libgm 的内部兜底——**上游 Core3D 直接把同一个表达式算好再传进来**（§11.3 `CSG_BasicDIS`），也就是说 PDMS 的 `RADI` 属性根本没进 libgm，只被用作"走椭圆封头还是走球缺"的开关。

### 5.8 `GM_CircTorus`（`gmccto`）— @ 0x10028760

字段：`+5 = rInside`，`+6 = rOutside`，`+7/+8 = start/finish angle°`。**存的是内外半径，不是"中心线半径 + 管半径"**——后两者是在 `calcFacets` 里现算的：

```
rTube   = (rOutside − rInside)·0.5          // 管（截面）半径
rCentre = rOutside − rTube = (rOutside + rInside)·0.5
nRing    = d2_numberOfSegmentsForPartRev(rOutside, arcTol, &start, &finish, &isFull)
nProfile = d2_numberOfSegmentsForCircle(rTube, tol)
dRing = (finish − start)·π/180 / nRing
dProf = 2π / nProfile
顶点(i,j): β = j·dProf,  α = start·π/180 + i·dRing
   R = rTube·sin β + rCentre
   P = ( R·cos α,  R·sin α,  rTube·cos β )
纵向边 type = 可变（`v132`，整圈/端口不同）；环向边 type = 1
isFull == false 时额外补两个端面
```

> 早先记的"环向段数用 `rProfile`、截面段数用 `rTorus`，看起来是反的"是**字段命名错误导致的误读**，已订正：环向段数用 `rOutside`（弧上离轴最远的那条母线，取它是保守且正确的），截面段数用 `rTube`，两者都不交叉。对 gen-model 的直接影响见启示 15。

### 5.9 `GM_RectTorus`（`gmcrto`）— @ 0x1005e2f0

每个角站位 4 个顶点（矩形截面四角），`GM_Facets(2n+2, 4(n+nSeg), 4n)`；整圈时首尾缝合，部分回转时补两个端面。

### 5.10 `GM_Pyramid`（`gmcpyr`）— @ 0x1005cd50

`GM_Facets(6, 12, 8)`，但**每条棱都有退化分支**：底面 x/y 半长、顶面 x/y 半长各自与 `1e-6` 比较，为零时那条边塌成点或线，面数相应减少。所以"棱锥/棱台/楔形/三角柱"共用一个类。`xShift/yShift` 使顶面整体平移。

### 5.11 `GM_Extrusion`（`gmcxtr`）— `calcFacets` @ 0x10032ec0

由 `GM_Profile` 驱动：

```
D2_Profile::getAllSpans() → 逐 span 展成 D2_LabelledPolygon（弧按 nSteps 展开）
顶点：轮廓点 (x,y,0) 与 (x,y,height) 成对
侧边：addEdge(i, i+1, label, spanType)、addEdge(i, i', 2)、addEdge(i+1, i'+1, 2)
若 surfacesWanted:
   直线段 → addSurface(1 /*plane*/, …)
   弧段(|bulge| ≥ 3.06e-5) → 先用 D2_Circle::coincidentWithinTolerance(…, normtol)
                             在已建曲面里找同一个圆柱，找不到才 addSurface(2 /*cylinder*/)
两个端盖：addSurface(1, …) + addFacet
```

**同一个圆柱面被多段弧共享**——这直接决定了后面法线平滑的分组（§8.3）。

### 5.12 `GM_ExtrusionGroup` — `calcFacets` @ 0x100342a0 + `GM_ProfileTessellator`

唯一用到 OpenGL GLU 的地方：

```c
// sub_1005AE80 = GM_ProfileTessellator::ctor
tess = gluNewTess();
gluTessCallback(tess, GLU_TESS_BEGIN  /*100100*/, …);
gluTessCallback(tess, GLU_TESS_VERTEX /*100101*/, …);
gluTessCallback(tess, GLU_TESS_END    /*100102*/, …);
gluTessCallback(tess, GLU_TESS_COMBINE/*100105*/, …);
gluTessProperty(tess, GLU_TESS_WINDING_RULE  /*100140*/, GLU_TESS_WINDING_POSITIVE /*100132*/);
gluTessProperty(tess, GLU_TESS_BOUNDARY_ONLY /*100141*/, GL_TRUE);
gluTessNormal(tess, 0, 0, 1);
```

`BOUNDARY_ONLY = TRUE` ⇒ **不是拿 GLU 做三角化，是拿 GLU 做 2D 多边形并集**：把一组轮廓按 POSITIVE 缠绕规则合并成干净的外/内环，再自己拉伸。`COMBINE` 回调处理自交产生的新点。断言在 `GM_ProfileTessellator.cxx:264`。

### 5.13 `GM_Revolution`（`gmcrev`）— @ 0x10060260

```
GM_Profile::polygonForFacet()  → D2_Polygon
translatePolygonIntoStandardPosition()   // 平移到标准位
movePointsOntoYAxis()                    // 贴到轴上（消除轴上重复点）
calcMaxRadiusOfRevolution(poly)
n = d2_numberOfSegmentsForPartRev(rMax, tol, &start, &finish, &isFull)
超限 → GM_Revolution::printLimitFacetWarning(...)
整圈 / 部分（补端面）两条分支
```

### 5.14 `GM_SweptSolid` 家族与 `GM_Collar`（`gmcrsl`）

`GM_SweptSolid`（type 18）是 `GM_Extrusion` / `GM_ExtrusionGroup` / `GM_Revolution` / `GM_Collar` 的共同基类：持有一个**外轮廓** `getOuter()` + N 个**内轮廓** `getInner(i)`（洞），每个带一个 `GM_ProfileSense`（`getSense(i)`），`addElement(GM_Profile*)` 追加。

`GM_Collar`（type 17）就是 `gm_CreateRuledSolid(height, baseProfileId, topProfileId)` 建的东西——**上下两个不同轮廓之间的直纹体**（ruled solid）。构造是 `GM_Collar(height, base, top, 0.1)`，GML 助记符 `gmcrsl`。

`calcFacetsWithoutSurfaces` @ 0x100299e0 的分工：

| 私有方法 | 作用 |
|---|---|
| `setSpanSteps()` @ 0x1002b3c0 | 对上下两个轮廓的**每一对 span** 取半径较大者算 `d2_numberOfSegmentsForCircle`，两端强制同段数——否则上下点数对不上就没法拉直纹 |
| `linkedProfiles()` / `otherEnd(p)` | 维护"哪两个轮廓是一对" |
| `formBaseFacet(poly, …)` | 底盖 |
| `formTopFacet(poly, …, height, …)` | 顶盖 |
| `formTopSides(poly, …, height, …)` | 侧壁直纹面 |

`changed(GM_Item&)` 说明它会跟随轮廓变化重算。这个类明显服务钢结构/异型过渡件（PDMS 里的 collar/transition）。

### 5.15 `GM_Polyhedron` / `GM_MeshSurface`

不做三角化，直接接收外部数据：`gm_CreatePolyhedron()` + `gm_AddVertexToPolyhedron` + `gm_AddFacetToPolyhedron` + `gm_AddSideToFacetOfPolyhedron(facet, v, GM_EdgeType, …)`；或 `gm_AddFacetMeshData(id, points, normals, indices)` 一次灌入。`gm_SetGeometricalValidationTolerance` 控制校验严格度。这就是 Core3D 塞入"外部几何"的入口。

---

## 六、`GM_Facets` —— 面片集数据结构

### 6.1 布局（32 位，`sizeof = 0xC8`）

| 偏移 | dword 下标 | 内容 |
|---|---|---|
| `+0x00` | 0 | vftable |
| `+0x04` | 1 | 缓存的 `D3_Limits*`（懒算；任何 `addVertex` 都会 `delete` 掉它） |
| `+0x08` | 2 | 标志字节（`addFacet` 读它决定新面的默认朝向） |
| `+0x0C` | 3 | `= 3`（初值） |
| `+0x10` | 4 | **`normalisationStage_`**（0 / 1 / 2） |
| `+0x14` | 5,6,7 (+heap 8) | **facets** `vector<GM_SuperFacet*>` |
| `+0x24` | 9,10,11 (+heap 12) | **edges** `vector<GM_Edge*>` |
| `+0x34` | 13,14,15 (+heap 16) | **vertices** `vector<D3_Point*>` |
| `+0x44` | 17,18,19 (+heap 20) | **normals** `vector<D3_Vector*>` |
| `+0x54` / `+0x58` | 21 / 22 | facet 限界数组指针 / 其长度（`−1` = 未建，`buildFacLimArray()` 填） |
| `+0x5C` | 23,24,25 (+heap 26) | lines（`GM_Line*`，消隐用） |
| `+0x6C` | 27,28,29 (+heap 30) | faces（`GM_Face*`，消隐用） |
| `+0x7C` | 31,32,33 (+heap 34) | 排序缓存（`sortVertices/sortFacets/sortCurveEdges` 的结果，`addVertex` 会清空） |
| `+0x8C` | 35 | **surfaces**：`vector<GM_Surface*>*`（指针；无曲面时 NULL） |
| `+0x90` / `+0xA0` | 36..38 / 40..42 | 另两组向量（+heap 39 / 43） |
| `+0xB0` | 44（字节） | 标志 |
| `+0xB8..0xC4` | 46..49 | 侵入式链表哨兵 + `FL_NodeHeap` 句柄 |

`sizeof(GM_Facets) = 0xC8`。`GM_Facets(nF, nE, nV)` 只做三次 `reserve`（分别对 +0x14 / +0x24 / +0x34）。

### 6.2 三个元素

```c
GM_Edge (0x1C):  +0 owner  +4 iV1  +8 iV2  +0x0C iFacetL(-1)  +0x10 iFacetR(-1)  +0x14 label  +0x18 type
GM_Surface(0x38): +0 type  +4 D3_Point(24B)  +0x1C D3_Vector(24B)  +0x34 label
GM_SuperFacet(0x3C): owner, sideList(vector<int>), holeList, iSurface/label, plane*, state, …
```

### 6.3 **边的方向编码：`2·edgeIndex + side`**

`GM_SuperFacet` 的边表里存的不是边下标，而是 `2·i` 或 `2·i+1`，低位是走向。反编译里到处是这个模式：

```c
if (e % 2) e2 = e - 1; else e2 = e + 1;   // 反向
```

一条边被两个面共用时，两侧存的 side 位相反 —— 这就是"半边（half-edge）"的紧凑写法。`GM_Facets::iFacetLForSide(int)` / `getEdgePtrForSide(int, GM_Edge*&, int&)` 是它的访问器。

### 6.4 `GM_EdgeType` 枚举（名字来自 `GM_Edge::printOn` 的 switch）

```c
bool isCurve()       { return type == 4; }
bool isVisible()     { return type != 1 && type != 5 && type != 6; }
bool isSurfaceEdge() { return type != 0 && type != 4; }
```

| 值 | `printOn` 打印的名字 | 可见 | 曲面边 | 何时产生 |
|---|---|---|---|---|
| 0 | `Intersection` | ✔ | ✘ | 交线（非曲面边） |
| **1** | `Invisible` | ✘ | ✔ | **平滑**：圆柱竖缝、球/碟/环全部边 |
| **2** | `Visible` | ✔ | ✔ | **硬**：立方体全部边、圆柱端圈 |
| 3 | `Set-op intersection` | ✔ | ✔ | 布尔运算沿交线新建的边 |
| 4 | `Curve` | ✔ | ✘ | `GM_Arc` / `GM_Straight` / `GM_Bezier` 的线框边 |
| 5 | `Invisible silhouette` | ✘ | ✔ | 消隐阶段标记 |
| 6 | `Back invisible` | ✘ | ✔ | 消隐阶段标记 |
| 7 | `Visible silhouette` | ✔ | ✔ | 消隐阶段标记 |
| 8 | `Back visible` | ✔ | ✔ | 消隐阶段标记 |

0–4 由建模阶段写入，5–8 由 `HL_*` 消隐阶段回写。
**"可见"= 出图会画出来的硬边，同时也是法线平滑的分组边界**（§8.3），也是 `AM_SGL` 索引流里顶点符号位的来源（§7.1）。

### 6.5 内容类别（`GM_Facets + 0x0C`）

```c
bool isWireframe() { return kind <  2; }
bool isSheet()     { return kind == 2; }
bool isSolid()     { return kind == 3; }
```

构造时默认 3（实体）。`gm_combine` 会把 A 的 kind 复制给结果，并用 `kind > 1` 决定要不要处理 B 侧（线框只被切，不参与双向分类）。

### 6.6 `GM_SurfaceType`

| 值 | 类型 | `point` | `vector` |
|---|---|---|---|
| 1 | 平面 | `normal × (−d)`（垂足） | 单位法线 |
| 2 | 圆柱 | 轴上一点 | **轴单位向量 × 半径** |

`addFacetAndFlatSurface(loop, label, reqs)`：不要曲面时等价于 `addFacet(loop, label, 5)`；要曲面时先 `GM_SuperFacet::setPlane()` 算平面，再 `addSurface(1, …)`，并且**把 facet 的第二个 int 从 label 改成 surfaceIndex**。所以 `GM_SuperFacet` 的那一格是 label / surfaceIdx 复用的。

### 6.7 顶点焊接

```c
// GM_Facets::vertexAt(point, tol)  @ 0x10040e70
先用 [p−tol, p+tol]³ 的 AABB 粗筛，再 GM_Near::pointNearPoint(a, b, tol)
命中返回既有下标，否则 addVertex 新建
```

**线性扫描**，没有空间索引。`vertexAtExact` 是精确比较版。布尔运算里大量调用它——这是 libgm 在大模型上的主要性能悬崖之一。

### 6.8 归一化 `normalise(normTol, tangTol, stage)`

两阶段，`normalisationStage_` 记录进度，幂等：

```
stage 1: check() → adjustVertices(normTol)      // 顶点吸附
                 → doFacetCancellation(normTol) // 反向重合面对消
                 → squeezeEdges / squeezeVertices / squeezeFacets / squeezeSurfaces
         返回值 = 顶点数是否变化
stage 2: normaliseStage2(tangTol)               // 按相切容差合并共面/相切面
```

#### 6.8.1 `doEdgeCracking(tol)` @ 0x10041230 —— 边打断

把被别的面顶点打断的边拆成多段，是保证布尔后网格拓扑封闭的关键步骤。算法：

```
for each facet F[i]:
    limF = F[i].limits.expandBy(tol)
    crackEdgesOfFacet(i, i, tol)               // 自检：自身顶点落在自身边上
    for j in 0..i-1:
        if F[j].limits ∩ limF ≠ ∅:
            crackEdgesOfFacet(i, j, tol)        // 互检：F[j] 的顶点落在 F[i] 的边上
```

O(n²) 双循环，但每一对先走 AABB 粗筛。

`crackEdgesOfFacet(iA, iB, tol)` 是内层核心（0x100417b0）：
- 取两个面的边（端点对），逐对检查：
  1. **点近线**：`GM_Near::pointNearLineQuick(v, p0, p1, tol)` + `GM_Near::pointInCircle(v, p0, p1)`（点在端点间、不越界）。命中说明 B 的顶点 v 落在 A 的边 p0→p1 上。
  2. **线近线**：`GM_Near::lineNearLineQuick(q0,q1, p0,p1, tol, &hitPt)` —— 两条边最近点。命中后把最近点匹配到已有顶点（`pointNearPoint`），匹配不上就 `vertexAt` 焊接。
- 收集所有打断点后，逐边调 `crackEdge` 拆开。

`crackEdge(edge, crackPoints)` @ 0x10042210：
- 对 crackPoints 沿边方向**投影排序**（选择排序，4 路展开）：`t = (P − P0) · (P1 − P0) / |P1 − P0|²`。
- 逐点把原边一分为二：新建 `GM_Edge` 拷贝 label / type / facet 归属，插进左右两个面的 sideList。
- 每次拆边都使被影响面的 limits 和 plane 缓存失效。

辅助函数：`resolveFacet`（0x10041100）处理面内自交——一个面被自身的边打断后需要分裂成多个面；`separateInsideOutPart`（0x10043500）做分裂；`mergeNeighbour`（0x10043a80）合并共边邻居；`enclosureOfLine`（0x10044c40）判断一条线段是否被另一个面完全包围。

#### 6.8.2 `doFacetCancellation(normTol)` @ 0x100429b0 —— 反向面对消

**核心判据：两个面的法线夹角 ≥ 175° 时对消**（即几乎反向、偏差 ≤ 5°）。

```
for each facet F[i]:
    resolveFacet(i, tol)
    limits[i] = F[i].limits.expandBy(tol)
    normal[i] = F[i].plane.normal
    for j in 0 .. min(i, a4)-1:          // a3/a4 是跳过范围（布尔后可能跳旧面）
        if limits[j] ∩ limits[i] = ∅: skip
        if angle(normal[i], normal[j]) < 175°: skip          // ★ 硬编码 175°
        crackEdgesOfFacet(i, j, tol)                          // 保证拓扑配对
        if cancelFacets(i, j, tol):                            // 尝试对消
            更新 limits[i], normal[i]
            可能重新分配面（addFacet + reAssignAllSides）
```

`cancelFacets` @ 0x10043ce0：
- 逐边调 `enclosureOfLine` 判断边是否被对方面完全包围。
- 未完全包围 → 返回 false（不能对消）。
- 全部包围 → 把重合边从一个面移到另一个面，边的 type 设为 `5`（如果不是 3 或 4），实现面的局部抵消。

**对齐启示**：175° 是一个不可配置的常数，对应 5° 的容许偏差。我们如果要做后布尔清理（消除布尔产生的反向重叠面），应该照搬这个门槛。

#### 6.8.3 `normaliseStage2(tangTol)` @ 0x10045400 —— 布尔边定型

决定布尔产生的 `Set-op intersection`（type 3）边最终变成硬边还是平滑边：

```
for each edge e where e.type == 3:
    fL = e.iFacetL;  fR = e.iFacetR
    if 只有一侧: 取有侧面的 surfaceIndex，e.type = 2（硬边）
    if 两侧都有:
        surfIdx = max(surfaceIndex(fL), surfaceIndex(fR))
        e.label = surfIdx
        if tangTol <= 0.0 或 isTangentDiscontinuity(fL, fR):
            e.type = 2      // 硬边
// 第二遍：仍然是 type 3 的 → type 1（平滑边）
for each edge e where e.type == 3:
    e.type = 1
if debugLevel > 1: check()
```

`isTangentDiscontinuity`（`sub_10044F60`）是一个**固定 22.5°** 的几何判据：
- 预算 `cos(22.5°)`, `sin(22.5°)`, `tan(45°)` 等常量（懒初始化，flag 位控制）。
- 取两面法线差向量 `Δn = n_B − n_A`，差向量模 `|Δn|`。
- `|Δn|` > `2·sin(22.5°) / √(cos⁴(22.5°) + sin²(22.5°)) + 0.001 ≈ 0.818 + 0.001`：**一定不切** → 返回 1（不连续，设硬边）。
- `|Δn|` < 0.001：**几乎共面** → 返回 0（连续，设平滑边）。
- 中间地带：构造一个变换把法线差映射到局部坐标系，然后检查**边周围相邻面**的法线经过该变换后是否超出阈值。若任一邻居超出则返回 0（设平滑边，因为与周围面之间有更大的不连续处），否则返回 1。

这段判据的效果是：布尔切出来的交线如果穿过一个曲面（如圆柱被平面截断），交线处两侧面法线偏差小 → 设为平滑边 → 着色过渡自然。如果穿过一个尖角（两个平面的交），偏差大 → 设为硬边 → 保持清晰棱线。

---

## 七、出料：`GM_Facets → AM_Body → AM_SGL`

```
GM_Facets                      多边形面片 + 半边 + 解析曲面
   │  AM_Body::AM_Body(const GM_Facets&)      @ 0x100056e0
   │     addSimpleFace()   单环平面面
   │     addGeneralFace()  带洞 / 需要 D2_LabelledRegion 的面
   ▼
AM_Body                        真 B-rep：Face / Loop / CoEdge / Edge / Vertex / Surface
   │  AM_SGL::AM_SGL(const AM_Body&)          @ 0x1000d6e0
   │     逐 Face → Loop → CoEdge，惰性 calcStartNormal()
   ▼
AM_SGL   +0x00 vector<D3_Point*>  vertices
         +0x10 vector<D3_Vector*> normals
         +0x20 vector<int>        formation data   ← 索引流
```

对外：

```c
gm_QueryFacetDataSize(id, &a, &b, &c);
// a = GM_Facets 的 facet 数, b = 顶点数, c = 边数   ← 直接从 GM_Facets 三个 vector 长度读
gm_QueryFacetData(id, vector<D3_Point>&, vector<D3_Vector>&, vector<int>&);
// 现场构造 AM_Body + AM_SGL，输出 (点, 法线, formation data)
```

两个函数**量纲不同**：`Size` 报的是 `GM_Facets` 的面/顶点/边计数，`Data` 交付的是 `AM_SGL` 的点/法线/索引流。调用方只能拿 `Size` 当 reserve 提示，不能当精确长度。（**未与运行时对拍，标记为观察**。）

`id` 必须是 `GM_SolidUserFacets(37)` / `GM_ComplexUserFacets(13)` / `GM_3DGeomUserFacets(48)` 之一，否则 `gm_message(2000, "… Invalid Solid/Complex/3D Geom Facet Structure id")`。

### 7.1 ★ `formation data` 索引流编码（已解）

`AM_SGL::AM_SGL(const AM_Body&)` @ 0x1000d6e0 的输出格式，从反编译逐条读出来：

```
// 第一遍：把 body 的顶点原样拷进 AM_SGL.vertices，并建 vertex* -> 下标 的映射
for each Face f of body:
    for (j = 0; j < f.loopCount(); ++j):
        L = f.loop(j)
        n = L.coEdgeCount
        push( j == 0 ? +n : -n )                    // ★ 环头：外环 +n，内环（洞） -n
        ce = L.firstCoEdge
        repeat n times:
            if ce.startNormal == (0,0,0): ce.calcStartNormal()   // 惰性算平滑法线
            ni = normals.findOrInsert(ce.startNormal)            // 精确去重
            push( ni + 1 )                                       // ★ 法线索引，1-based
            vi = vertexIndexOf(ce.startPoint)                    // 越界则 AM_SGL.cxx:101 内部错误
            push( ce.edge.isVisible() ? (vi + 1) : -(vi + 1) )   // ★ 顶点索引，1-based，
                                                                 //   符号 = 该顶点起始的边要不要画
            ce = ce.next
        断言 ce 回到起点，否则 AM_SGL.cxx:106
```

三条要点：

1. **`±n` 的环头**：正数开一个新面的外环，负数是同一个面的洞。面与面的分界就是"下一个正数"。这就是为什么 libgm 能把带洞的平面（比如开孔钢板）一次交付而不用先三角化。
2. **每个角点是一对 `(法线索引, ±顶点索引)`**，都是 **1-based**（所以 0 永远不出现，可用作哨兵）。
3. **顶点索引的符号 = 边可见性**（`AM_Edge::isVisible`，来自 `GM_EdgeType`）。这就是 GINO/SGL 的 edge-flag 约定：负号表示"从这个角点出发的那条边是平滑边，出图时不要画"。我们要复刻 AVEVA 的线框/消隐效果，这个位不能丢。

法线表通过一个按向量精确比较的查找结构去重（命中就复用已有下标），所以 `normals.len()` 通常远小于角点数。`AM_SGL::sizeOfFormationData()` / `formationComponent(i)` / `vertexCount()` 是它的公开访问器。

相关偏移：`AM_CoEdge` `+0x00` 起 24 B 是 `startNormal`（`D3_Vector` 内联）、`+0x18` `startPoint(D3_Point*)`、`+0x1C` `next`、`+0x24` `loop`、`+0x28`(字节) `isCoEdgeA`、`+0x2C` `edge`；`AM_Edge +0x08`(字节) `visible`；`AM_Face +0x24` loops 向量。

---

## 八、法线与平滑着色

### 8.1 `AM_CoEdge::calcStartNormal()` @ 0x1000c390 —— 平滑组算法

```
group = [this]
cur = this
loop:
    e = cur->edge
    cur = (cur.isCoEdgeA ? e.coEdgeB : e.coEdgeA)->next   // 绕顶点走到相邻面
    if cur == null or cur == start: break
    if cur->edge->isVisible(): break                      // ★撞到硬边就停
    group.push(cur)
（再反方向绕一遍）

if group.size() == 1:
    normal = 该面的平面法线
else:
    normal = normalise(Σ 各面平面法线)                    // 4 路展开的求和循环
    if 求和为零向量: normal = 常量兜底 (dword_100AFC08)
把同一个 normal 写回 group 里的所有 co-edge
```

**即：顶点法线只在"由不可见（平滑）边连通"的面之间求和**。硬边（`type = 2`）把顶点周围的面切成若干平滑组，每组一个法线 —— 这正是圆柱侧面光滑、端圈锐利的实现。

### 8.2 谁决定边是硬是软

由**图元自己在 `calcFacets` 里传的 `GM_EdgeType`** 决定，不是运行时按夹角判：

- `GM_Block` / `GM_Pyramid`：全 `2`（硬）。
- `GM_Cylinder`：端圈 `2`，竖缝 `1`。
- `GM_Sphere` / `GM_SDish` / `GM_EDish` / `GM_CircTorus`：全 `1`。
- `GM_Extrusion`：直线段边 `2`，弧段边随 span 类型。

`tangtol_ = 5°` 用在 `normaliseStage2` 的**面合并**，不在这里。

### 8.3 曲面共享的连带效果

`GM_Extrusion` 用 `D2_Circle::coincidentWithinTolerance(other, normtol)` 把同一圆柱上的多段弧合并到一个 `GM_Surface`。曲面共享 → `AM_Surface` 共享 → `AM_Face` 归到同一 surface，平滑组因此跨越 span 边界。

---

## 九、CSG 布尔：`GM_SetOp.cxx`

### 9.1 `GM_Operation` 枚举

从 `gm_CreateCombination` 的分派与 `gm_combine` 的标志推导：

| 值 | 含义 | `gm_CreateCombination` 建的类 |
|---|---|---|
| 0 | 聚合（无交线） | `GM_AggregateCombination` |
| **1** | **UNION** | `GM_SolidCombination` |
| **2** | **INTERSECTION** | `GM_SolidCombination` |
| **3** | **DIFFERENCE** | `GM_SolidCombination` |
| 4 | 聚合 + 画交线（`GM_CompFacets::aggregateWith` 里 `if (op == 4) addIntLines(...)`） | `GM_AggregateCombination` |
| 5 | 复合（非实体） | `GM_ComplexCombination` |

其它值 → `gm_message(2011, "GMCCOM/gm_CreateCombination - Invalid operation type")`。
`gm_CreateSection(id, op, cutType)` 只接受 `op ∈ {2, 3}`。

`GM_CutType`（名字来自 `GM_Section::printOn`）：**`0 = GM_CUTALL`**（切所有内容）、**`1 = GM_ONLYSOLIDS`**（只切实体，线框/片体原样保留）。

### 9.2 顶层驱动 `GM_SolidCombination::calcFacets` @ 0x10068170

```
acc = 第 0 个成员的面片集
for i in 1..n-1:
    b   = getOperandFacets(i)                       // 断言 isSolid()，否则 GM_SolidCombination.cxx:122
    out = new GM_Facets(nFacetsA+nFacetsB+1, nEdgesA+nEdgesB+1, nVertA+nVertB+1)
    gm_combine(op, acc, b, out)                     // 断言成功，否则 :142
    acc.release(); b.release(); acc = out
if op != UNION 且成员数 > 1:
    acc->normalise(normtol_, tangtol_, stage = 1)
```

**左折叠，两两做**。中间结果不做归一化，只在最后一次对非并集做 stage-1。

### 9.3 布尔核 `gm_combine`（`sub_10064140`）@ 0x10064140

```c
out.terminate(); out.normalisationStage = 2; out.surfaces = NULL;

// 按 op 设定"保留哪边 / 是否翻面"
if (op == INTERSECTION) { keepA=0; keepB=0; senseB=+1; flagA=0; flagB=1; }
else                    { keepA=1;          senseB=−1; flagA=1; flagB=0;
                          keepB = (op == UNION); }

// A、B 必须同时有/没有解析曲面，否则 internal fault GM_SetOp.cxx:549
if (A.hasSurfaces && B.hasSurfaces) out.ensureHasSurfaces();

// ★ 早退：包围盒不相交
if (!A.limits().intersects(B.limits())) {
    if (keepA) out.append(A);
    if (keepB) out.append(B);
    return;
}

out.appendSurfaceDataFrom(A, false);
out.appendSurfaceDataFrom(B, /*negateNormals=*/ op == DIFFERENCE);   // 差集翻 B 的法线

GM_IntCurve::create(intCurves, A, B);   // 面-面求交 → 交线集（GM_Intersect.cxx）

addRetainedVertices(A, B, isA=1, keepA, sense, out, retainA, vmapA);
if (A.kind > 1) addRetainedVertices(B, A, isA=0, keepB, sense, out, retainB, vmapB);
addIntCurveVertices(intCurves, flagI, ivMap, out, …);
addRetainedEdges  (A, retainA, vmapA, …, surfBase=0, out);
if (A.kind > 1) addRetainedEdges(B, retainB, vmapB, …, surfBase=nFacetsA, out);
addIntCurveEdges  (intCurves, ivMap, flagE, nFacetsA, out);
addRetainedFacets (A, out, 0);
if (A.kind > 1) addRetainedFacets(B, out, surfBaseB);

out.buildFacets();
out.normalisationStage = 0;
if (nIntCurves > out.nFacets) out.normalisePostSetOp(normtol_, …);
out.squeezeEdges(); squeezeVertices(); squeezeFacets(); squeezeSurfaces();
```

五个内部阶段的名字不是猜的，是每个函数入口 `FL_MonitorThisBlock` 的标签字符串：`addRetainedVertices` (0x10063200)、`addRetainedEdges` (0x10063400)、`addRetainedFacets` (0x10063CC0)、`addIntCurveVertices` (0x10063F30)、`addIntCurveEdges` (0x10064050)；`gm_combine` 本身的标签就是 `gm_combine`。

### 9.4 ★ 里/外分类：一个带符号的保留数

`addRetainedVertices` 是整个布尔的判据所在，只有十几行：

```c
for (i = 0; i < A.nVertices; ++i) {
    v = A.vertex(i);
    c = 0;
    if (v 在 B 的整体包围盒内)
        c = clashVtSo3D(v, B);                     // 包含数（见下）
        if (c >= 2 && debugLevel > 1)
            打点 "SET OP note: vertex containment number not 0 or 1: A/B <i> <c>"
    retain[i] = keepFlag + sense * c;              // ★ 唯一判据
    if (retain[i] != 0)
        vmap[i] = out.addVertex(v);
}
```

把 `gm_combine` 设的标志代进去：

| op | A 侧 `keep, sense` | `retain_A` | B 侧 `keep, sense` | `retain_B` | 含义 |
|---|---|---|---|---|---|
| 1 UNION | `1, −1` | `1 − c` | `1, −1` | `1 − c` | 各留在对方**外**的部分 |
| 2 INTERSECTION | `0, +1` | `c` | `0, +1` | `c` | 各留在对方**内**的部分 |
| 3 DIFFERENCE | `1, −1` | `1 − c` | `0, −1` | `−c` | A 留外、B 留内且**符号为负** |

差集里 B 侧的 `retain = −c` 为负，配合前面 `appendSurfaceDataFrom(B, negate = true)` 翻转法线，正好把 B 的内表面变成 A 的空腔壁。**一个带符号整数同时编码了"留不留"和"翻不翻面"**——这是整段代码最漂亮的地方。

`c ≥ 2` 说明点落在嵌套壳里（自相交或多壳），libgm 不报错，按深度继续算，只打一条 note。

### 9.5 点在实体内：`clashVtSo3D`（+Z 射线穿越计数）

```c
// sub_1004D7E0（A 侧）/ sub_1004E260（B 侧），监视标签 "clashVtSo3D"
count = 0
for each SuperFacet f of solid:
    if (!f.limits) f.setLimits();
    if (p.x ∈ [f.xmin, f.xmax) && p.y ∈ [f.ymin, f.ymax) && f.zmax > p.z) {
        hitZ = 0;
        n = facetCrossing(p, &hitZ);      // 带符号：按面朝向 ±1
        if (n && p.z < hitZ) count += n;  // 只数点上方的穿越
    }
return count;
```

即**从点沿 +Z 打一条射线，数带符号穿越次数**。XY 半开区间 `[min, max)` 的写法避免了射线正好擦过共享边时被数两次。内层 `facetCrossing`（0x1004D200 / 0x1004D630）遍历面的各边求交，同时维护"上方最近"和"下方最近"两个交点做插值；若上下穿越数不一致会 `gm_reportInternalFault("GM_Intersect.cxx", 287)`——那是"面没闭合"的自检。

### 9.6 交线

`GM_IntCurve::create` 的粗筛是 `D3_Limits::intersects`（整体 → 逐 facet 限界），细筛逐面对求交，用 `GM_IntPoint` / `GM_Line` 串成交线。**没有 BSP、没有八叉树**，是限界数组 + 双重循环。沿交线新建的边类型是 `3 = Set-op intersection`。

### 9.7 ★ `addRetainedEdges` @ 0x10063400 —— 边级碎片重建（已解）

在 `addRetainedVertices` 确定了每个源顶点映射到输出的哪些顶点之后，`addRetainedEdges` 重建边：

```
for each edge E[i] of source A:
    v1 = E[i].iV1;  v2 = E[i].iV2
    对两端各加 |retain| 次到 sub_10063130（计数验证？）
    n1 = 端点 v1 映射的输出顶点数（0、1、或由链表给出多个）
    n2 = 端点 v2 映射的输出顶点数
    if n1 ≠ n2 → throw GM_SetOpException     // ★ 强一致性：两端必须同数
    if n1 == 0: skip                           // 这条边完全在"被删"的一侧
    facetL = E[i].iFacetL + surfBase（如 ≥ 0）, 否则 -1
    facetR = E[i].iFacetR + surfBase（如 ≥ 0）, 否则 -1
    if n1 == 1:   // 简单情况：一对一映射
        addEdge(out, mappedV1, mappedV2, facetL, facetR, label, type)
    else:         // 多映射：边被交线打断成多段
        构建两个数组 vlistA[], vlistB[]（各 n1 个映射顶点）
        去除 A/B 之间的共同项（它们是"边上的交点"只算一次）
        如果去完之后为空 → skip
        ★ 沿主轴排序：选 (Σ出点方向向量) 的最大分量轴
            对 vlistA 和 vlistB 各做选择排序（4 路展开）
        按排序后的顺序配对 addEdge(out, vlistA[k], vlistB[k], ...)
```

三个要点：

1. **两端映射数必须相等**，否则抛 `GM_SetOpException`（GM_SetOp.cxx:55）。这是因为一条边被打断成 k 段时，两端各多出 k−1 个中间点——数目一致是拓扑正确的必要条件。
2. **排序轴**：先把 `vlistA` 和 `vlistB` 对应顶点的方向向量加到一起，取绝对值最大的分量（0=x, 1=y, 2=z）作为排序键。选择排序是经典的 MSVC 2010 时代写法，4 路展开是手工优化。这保证了被切成多段的边按空间顺序配对，不会交叉。
3. `surfBase` 偏移是 A/B 两侧曲面索引的区分：A 侧 surfBase = 0，B 侧 surfBase = nSurfacesA，这样两侧的面索引不会冲突。

### 9.8 `addRetainedFacets` @ 0x10063CC0 —— 面级复制

比边简单得多：

```
for each facet F[i] of source:
    surfIdx = F[i].surfaceIndex + surfBaseOffset
    newFacet = new GM_SuperFacet(0x3C)
    newFacet.owner = output
    newFacet.surfaceIndex = surfIdx
    拷贝 F[i] 的 sideList 模板（reserved 8）
    output.facets.push(newFacet)
    invalidate output.facLimArray
```

面的边列表不在这里重建——`addRetainedEdges` 在 `addEdge` 时已经写入了每条边的 `iFacetL` / `iFacetR`，`buildFacets()` 会从边的面归属重建每个面的 sideList。所以 `addRetainedFacets` 只做"占位"——为每个源面在输出里建一个空壳，后续 `buildFacets` 填充。

### 9.9 切割 `applyAsCutTo`

`GM_Solid::applyAsCutTo(facets&, trans, …, op, cutType, reqs)`：取自身面片 → `transform` → `reLabel(labelFrom, labelTo)` → 调目标的 `sectionBy(facets, op, cutType)`。`GM_CutSurface`（`gmccut`，由 `gm_AddCutProfilePoint` 定义的多边形棱柱）走自己的 `applyAsCutTo` 实现。

开放曲面遇到 section 时只警告不失败：`"GML warning: section failed with open surface (ignored)"`、`"GML warning: clash with open surface - limits estimated"`。

---

## 十、其他算法

- **质量属性** `gm_QueryMass(id, &area, &volume, &centroid, &inertia)` → `GM_Facets::massProperties` @ 0x10040430：逐 `GM_SuperFacet::massProperties` 累加（散度定理），最后把质心与惯性矩阵除以体积。传 `isSolid()` 标志决定是否按实体积分。
- **射线** `gm_QueryRay` / `gm_QueryXRay` → `findNearestRayHit` / `findAllRayHits`：`D3_Limits::intersectsLine` 粗筛 + 逐面片求交，取最近命中，返回命中点、命中面的 `D3_Plane` 和 label。
- **干涉** `gm_QueryClash(idA, transA, idB, transB, &point)` / `gm_QueryClose(..., dist, &point)` → `clashRegionWith` / `closeTo` / `surfaceCloseTo`，走 `D3_LimitsArray::formCells()` 的**均匀格子**加速。
- **`GM_Verge`** —— 消隐模块的轻量边引用。布局 `sizeof = 0x0C`：

  ```c
  struct GM_Verge {
      GM_Facets* owner;   // +0x00
      int iVertex1;        // +0x04
      int iVertex2;        // +0x08
  };
  ```

  只存宿主 facets 和两个顶点下标，不拷贝坐标。`vertex0()` / `vertex1()` 按下标从 `owner->vertices` 取 `D3_Point*`。`outsideRect(D3_Limits)` 只检查 X/Y（`min(v0.x,v1.x) > lim.xMax || max(v0.x,v1.x) < lim.xMin || …`），Z 不参与——因为消隐是在屏幕空间做的，Z 只用 `maxZ()` / `minZ()` 参与深度排序。`printOn` 输出 `"GM_Verge[iV1 iV2]"`。它是 `HL_SceneElement` 的成员，用在 `findFrontFacets` / `obscure` 路径里。

- **消隐出图** `gm_CreatePicture(id)` + 14 个 `gm_Picture*` → `HL_Picture` / `HL_Cell`。核心是屏幕空间的**轴对齐二分细分（2D kd-tree）**：
  - `HL_Cell::chooseSplitLine(&coord)` 返回轴（`0 = x`，`1 = y`）或 `−1`（不分）。**深度上限 20**（`HL_Cell::depth(parent) + 1 >= 0x14` 直接返回 −1）。候选分割线取自单元内各元素包围盒的边界坐标；打分是
    `score = (总数 − max(左侧数, 右侧数)) · cellSplitFactor − 跨界数`
    —— 前项奖励"能把元素分开"，后项惩罚"被切成两半要重复登记"。`cellSplitFactor` 就是 `gm_PictureCellSplitFactor(picture, f)` 写进 `GM_PictureReqs + 200` 的那个数，**它是平衡度与重复率之间的旋钮**。
  - `HL_Cell::split()` 按选中的线造两个子单元（各自一个 `D2_Limits`），分发元素，然后递归 `split(child0)` / `split(child1)`；不合法的分割线会命中 `HL_Cell.cxx:147` 自检。
  - 遮挡消解走 `findFrontFacets` / `obscure` / `obscureFaces` / `selfHide`，结果由 `gm_PictureDraw(picture, lineCallback, faceCallback)` 的两个回调吐出。`gm_PictureGapping` 控制断线间隙，`gm_PicturePerspective` 透视，`gm_PictureStyle(GM_HideOpt)` 选消隐模式，`gm_PictureSelectSuppressedLines(…, GM_LineType)` 选哪类线被抑制。
  - 消隐阶段会**回写 `GM_EdgeType` 5–8**（silhouette / back visible / back invisible），见 §6.4。
- **树操作** `gm_CreateClippedTree(id, limits)`（按盒裁剪整棵树）、`gm_CreateExpandedTree(id, dist, mode, tol, &n)`（每个图元的 `expand(double,int,double)` 虚函数做偏置/膨胀）、`gm_CompressTree`（`compressCSGTree` / `compressCSGLink` 折叠可合并的组合节点）。

---

## 十一、上游：Core3D 的 noun → `gm_Create*` 分派

前十节都在 libgm 里面看，这一节是**谁在调它、拿什么调**。

**版本口径**：全文以 3.1 为准（`D:\AVEVA\Everything3D3.1\` 下的 `Core3D.dll` / `libgm.dll` / `libgeom.dll`），2.10 作为对照。两版都拆过，差异见 §11.8。

2.10 与 3.1 的 Core3D 已逐条对拍，**§11.3 / §11.4 / §11.6 的全部结论两版一致**（13 个 `getPrimGeom` 的属性读取顺序、算术、分支判据等价；3.1 只是把 `RINS` 的 `if (x < 0) x = 0` 写成了 `fmax(x, 0.0)`）。唯一的差异在 §11.2 的注册表：3.1 多挂了 10 个 `CSG_TreeBuilderCat` 的 noun（`SUBCOM` 与九个 `HV*` 暖通件），**图元 noun 一个没变**。地址索引两版都列在附录里。

### 11.1 三层结构

```
DB_Element（PDMS 元素）
   └─ CSG_TreeBuilder::addPlug(noun, plug)        ← 按 noun 挂"树构建器"
        ├─ CSG_TreeBuilderPrimitive        正实体
        ├─ CSG_TreeBuilderNegativePrimitive 负实体（N 开头的 noun）
        └─ CSG_TreeBuilderCat / MyBox / …   目录件、包围盒等
   └─ CSG_BasicPrimitive::findPrimitive(noun)     ← 按 noun 取"图元构造器"
        └─ CSG_BasicXXX::getPrimGeom(DB_Element&) ← 读属性 → 调 gm_Create*
```

两张表都在 `CSG_PrimitiveUtilities::initialise()` 里一次性注册，且**注册前有守卫**：只有当 `NOUN_BOX` 尚未登记时才建全表（幂等初始化）。`findPrimitive` / `found` 就是查这张表，未登记的 noun 由 `found()` 返回 false，不会走到 `getPrimGeom`。2.10 用的是有序 map，3.1 换成了排序过的 `CSG_BasicPrimitive::primList_` 顺序表，查法变了、内容没变。

### 11.2 noun 注册表（`initialise` 全量）

正负成对注册，**正负两侧共用同一个 `CSG_BasicXXX`**——负实体与正实体的几何完全一样，差别只在树构建器（`CSG_TreeBuilderNegativePrimitive` 会把结果交给 `CSG_PrimitiveUtilities::addStandAloneNegative`）。

| noun（正 / 负） | 图元构造器 | 最终 `gm_Create*` |
|---|---|---|
| `BOX` / `NBOX` | `CSG_BasicBOX` | `gm_CreateBox` |
| `CYLI` / `NCYL` | `CSG_BasicCYL` | `gm_CreateCylinder` |
| `CONE` / `NCON` | `CSG_BasicCON` | `gm_CreateSnout` |
| `PYRA` / `NPYR` | `CSG_BasicPYR` | `gm_CreatePyramid` |
| `DISH` / `NDIS` | `CSG_BasicDIS` | `gm_CreateSphericalDish` / `gm_CreateEllipticalDish` / `gm_CreateCylinder` |
| `SNOU` / `NSNO` | `CSG_BasicSNO` | `gm_CreateSnout` |
| `CTOR` / `NCTO` | `CSG_BasicCTO` | `gm_CreateCircularTorus` |
| `RTOR` / `NRTO` | `CSG_BasicRTO` | `gm_CreateRectangularTorus` |
| `SLCY` / `NSLC` | `CSG_BasicSLC` | `gm_CreateSlopeEndedCylinder` |
| `EXTR` / `NXTR` | `CSG_BasicEXT` | `gm_CreateExtrusion` |
| `REVO` / `NREV` | `CSG_BasicREV` | `gm_CreateRevolution` |
| `POLYHE` / `NPOLYH` | `CSG_BasicPOL` | `gm_CreatePolyhedron` |

`CSG_BasicRUL` 存在（`gm_CreateExtrusion`）但**不在 `initialise` 的表里**，由别处挂接。

挂 `CSG_TreeBuilderCat`（目录件，不是图元）的 noun：2.10 只有 `NOZZ`；3.1 增加了 `SUBCOM`、`HVBRCO`、`HVFLAN`、`HVHACC`、`HVSADD`、`HVSPLR`、`HVSTIF`、`HVTPPO`、`HVSKIR`、`HVIDAM`——一个子组件加九个暖通件。**图元 noun 两版完全相同。**

> `NOUN_*` 在 Core3D 里只是从 `core.dll` 导入的 `DB_Noun const*` 指针，不是整数码。要拿数字用 §11.7 的 word 函数直接算（如 `word("EXTR") = 900968`），别去反查指针。

### 11.3 ★ 属性 → 参数的逐图元对照

这是 `teach/MISSION.md` 课 03 遗留第 3 条要的东西。左列是 PDMS 属性（`DB_Element::getDouble(ATT_*, 0)` 的读取顺序），右列是实参。前九行的参数全在元素自己的属性上；后四行要走子元素链，其中 `RUL` 和 `EXTR` 的回退路径共用 §11.4 那个轮廓装配器，另外三个见 §11.6：

| 构造器 | 读的属性（按序） | 调用 |
|---|---|---|
| `BOX` @ 0x10726a90 | `XLEN` `YLEN` `ZLEN` | `gm_CreateBox(XLEN, YLEN, ZLEN)` —— 原样，**不减半** |
| `CYL` @ 0x10726ca0 | `DIAM` `HEIG` | `gm_CreateCylinder(DIAM/2, HEIG)` |
| `CON` @ 0x10726b30 | `DTOP` `DBOT` `HEIG` | `gm_CreateSnout(DBOT/2, DTOP/2, HEIG, 0, 0)` |
| `SNO` @ 0x10727450 | `DTOP` `DBOT` `XOFF` `YOFF` `HEIG` | `gm_CreateSnout(DBOT/2, DTOP/2, HEIG, XOFF, YOFF)` |
| `PYR` @ 0x10726f90 | `XBOT` `YBOT` `XTOP` `YTOP` `HEIG` `XOFF` `YOFF` | 七个原样直传 |
| `CTO` @ 0x10726be0 | `RINS` `ROUT` `ANGL` | `gm_CreateCircularTorus(max(RINS,0), ROUT, 0, ANGL)` |
| `RTO` @ 0x10727140 | `RINS` `ROUT` `HEIG` `ANGL` | `gm_CreateRectangularTorus(max(RINS,0), ROUT, HEIG, 0, ANGL)` |
| `SLC` @ 0x107272d0 | `DIAM` `HEIG` `XTSH` `YTSH` `XBSH` `YBSH` | `gm_CreateSlopeEndedCylinder(DIAM/2, HEIG, XBSH′, YBSH′, XTSH′, YTSH′)` |
| `DIS` @ 0x10726d10 | `DIAM` `HEIG` `RADI` | 三分支，见下 |
| `EXT` @ 0x10726e50 | （子元素链）`HEIG` | 先试 `gm_CreateExtrusionGroup`，失败才 `gm_CreateExtrusion(profile, HEIG)` → §11.6 |
| `RUL` @ 0x10727220 | `HEIG` +（子元素链） | `gm_CreateExtrusion(profile, HEIG)` |
| `REV` @ 0x107270c0 | （子元素链） | `gm_CreateRevolution` ×N + `gm_CreateCombination(1)` → §11.6 |
| `POL` @ 0x10726f10 | （子元素链） | `gm_CreatePolyhedron` + 逐点逐边装配 → §11.6 |

四条能落进代码的硬规则：

1. **直径半径之分只在圆截面上**。`CYLI` / `CONE` / `SNOU` / `SLCY` 的 `DIAM` / `DTOP` / `DBOT` 要除以 2；`BOX` 的 `XLEN` 和 `PYRA` 的 `XBOT`/`XTOP` 是**全长**，原样传。
2. **`CONE` 就是零偏移的 `SNOU`**，走同一个 `gm_CreateSnout`。底半径在前、顶半径在后。
3. **两种环体的 `RINS` 都会被钳到非负**（`if (RINS < 0) RINS = 0`），`ROUT` 不钳。起始角恒为 0，`ANGL` 是终止角。
4. **`SLCY` 的四个剪切角先归一化到 (−90°, 90°]**：`> 90` 减 180，`< −90` 加 180。归一化后才传。四个角各自独立做，没有联动。

`CSG_BasicDIS` 的三分支（`R = DIAM/2`，`H = HEIG`）：

```c
if (H <= 0)      return gm_CreateCylinder(R, 1.0);        // 退化成 1 单位厚的圆片
if (RADI > 0)    return gm_CreateEllipticalDish(R, H, H / ((R − H)/sqrt(R² + H²) + 1));
                 return gm_CreateSphericalDish(R, H);
```

两个反直觉点：

- **`HEIG <= 0` 不报错也不返回空，而是造一个高度写死 `1.0` 的圆柱。** 单位随库走（PDMS 内部是 mm），所以是一片 1 mm 厚的圆盘。这是"平封头"的实现方式，不是错误路径。
- **`RADI` 只当布尔用。** 传给 `gm_CreateEllipticalDish` 的第三个参数不是 `RADI`，是由 `R`、`H` 现算的转角半径，公式与 libgm 自己的 `GM_EDish::knuckleRadiusToUse()` @ 0x10032080 默认分支完全相同（§5.7）。也就是说 PDMS 目录里填的转角半径值**到不了几何核**，只有它的正负号有意义。

### 11.4 轮廓：`LOOP`/`VERT` → `D2_Profile`

`EXTR` / `RUL` / `REVO` / `POLYHE` 的形状来自子元素链，由 `DB_Create_D2_Profile`（`MTR_Entry` 标签）装配，再交给 `gm_CreateProfile`。

装配器按当前元素的 `TYPE` 分三路：

| `TYPE` | 走法 |
|---|---|
| `SPRO` | 解析路径：读 `PLAXCOS` / `PLAXSIN` / `IERR`，直接构造标准截面，不走顶点链 |
| `EXTR` `NXTR` `SEXT` `NSEX` `REVO` `NREV` `SREV` `NSRE` `PANE` | 下钻到子元素，找 `LOOP` / `SLOO` / `PLOO`，走下面的顶点链 |
| 其它 | 返回空轮廓 |

顶点链这条路有六道门：

1. **遍历顶点**，每个顶点收一个 2D 坐标和一个转角半径（PDMS 的顶点圆角）。
2. **定向归一化**：边走边累加叉积得带符号面积 `Σ`。`Σ <= 0` 时**倒序重走一遍**顶点表，即输出的 `D2_Profile` 恒为逆时针。
3. **质心平移**：先取顶点算术平均作原点，所有点减去质心后建轮廓，最后一次 `D2_Profile::moveBy(centroid)` 搬回去。纯数值调理——远离原点的大坐标轮廓不会在求交时丢精度。
4. **逐角圆角求解** `sub_10690FC0(prev, cur, next, radius, &P1, &P2, &bulge)`：给出圆角的两个切点和圆弧的 bulge。随后
   `dist(last, P1) > tol → addSpan(0.0, P1)`（直段），`dist(P1, P2) > tol → addSpan(bulge, P2)`（弧段）。
   **`tol` 是相对量：`maxCoord · 1e-5`**，不是绝对容差。
5. **闭合**：末点与首点距离在 `tol` 内就**直接改写成首点坐标**再 `D2_Profile::close()`，不留一条微小缝。顶点数必须 > 2。
6. **成品校验**：`|D2_Profile::area()| < 1e-6` 或跨度数 < 2 → 直接返回 0（不建对象）；建成后 `gm_ValidateObject` **必须返回 4 或 5**，其余取值一律 `gm_RemoveObject` 掉再返回 0。

第 6 步是个值得抄的形状：**先建、再验、验不过就删干净并返回"没有"**，不把半成品放出去。注意它的失败上报只有一句 `std::cout`，且要满足特定 noun 条件才打印——典型的静默降级（对照启示 5）。

### 11.5 摆放与开洞：`CSG_TreeBuilderPrimitive::getCSGTree`

@ 0x10726890，正实体的完整流程：

```
if (!CSG_TreeBuilderOptions::isWanted(elem, &label)) return 0;   // 过滤 + 取 label
gm_SetDefaultLabel(label);                                        // 下面所有 gm_Create* 都带这个 label
noun = elem->hardType();                                          // 注意是 hardType，不是 type
id   = findPrimitive(noun)->getPrimGeom(elem);                    // §11.3
elem->getAtt(...);                                                // 取定位/朝向
if (options.wantHoles) {
    id = addHolesBelowPrimitive(elem, id, options);               // 本体下挂的负实体
    if (owner is TMPL) {
        if (owner-of-owner is FIXING)                             // FIXING → 遍历它下面所有 TMPL
            for (t in TMPL under FIXING) id = addHolesBelowTemplate(elem, t, id, options, 4);
        else
            id = addHolesBelowTemplate(elem, owner, id, options, 4);
    }
}
```

三点：

- **`gm_SetDefaultLabel` 先于建图元**，label 是 libgm 侧的全局默认值（§4），不是逐调用参数。多线程建几何时这是一个共享状态。
- **用 `hardType()` 而不是 `type()`** 取 noun，即取实际存储类型而非逻辑/代理类型。
- **开洞是"顺着 owner 往上找模板"**：图元自己的负实体先减，再看它是不是挂在 `TMPL` 下；`TMPL` 的 owner 若是 `FIXING`，则该 `FIXING` 下**所有** `TMPL` 的洞都要减一遍（螺栓孔组）。

负实体路径 `CSG_TreeBuilderNegativePrimitive::getCSGTree` @ 0x10726770 更短：同样 `isWanted` + `SetDefaultLabel` + `getPrimGeom`，然后 `addStandAloneNegative`；并且有一条专门分支——**owner 的 `hardType()` 是 `BPANEL` 时把传出的变换清零**（面板上的开孔用面板自己的坐标系，不叠加元素变换）。另外，负实体在 `options.wantHoles` 为假时**整个返回 0**，即"不要洞"时负实体连几何都不建。

### 11.6 三个"带子元素"的图元：`EXTR` / `REVO` / `POLYHE`

前面 §11.3 的表在这三行只写了"（成员几何）"，这里补齐。三者的共同点：**参数不在元素自己的属性上，而在子元素链里**，所以 `getPrimGeom` 只是 `db_go_to_element` + 调一个专用装配器。

`DB_Element::getDouble(ATT_*)` 那套具名属性到这一层就换成了**数字 word 码**（`sub_1068EBC0(码)` 取类型、`sub_1068EBF0(码)` 取实数、`sub_1068ED00(码, 3)` 取三分量）。这些码已经全部解出来了，解法见 §11.7。

#### `EXTR` —— 多环走 `gm_CreateExtrusionGroup`

`CSG_BasicEXT::getPrimGeom` 先试那个多环装配器：

```
height = getReal(HEIG)
逐个 LOOP 子元素：ATT_OHTYPE 区分外环 / 洞环
          每环建一个 D2_Profile，跨度数 < 2 或 |area| < 1e-6 → 丢弃该环
id = gm_CreateExtrusionGroup(profiles, height)
gm_AddCurve(profile, id)                 // 线框用
```

失败（返回 < 0）才回退到 §11.3 那条 `gm_CreateExtrusion(单轮廓, HEIG)`。也就是说**带洞的挤出在 AVEVA 侧是 `ExtrusionGroup` 而不是"挤出后再布尔减"**——洞是轮廓层的内环，不是 CSG 减出来的（对照 §5.12 `GM_ExtrusionGroup` 用 GLU 三角化带洞平面）。

#### `REVO` —— 角度的三条约定 + 一个模块相关的对半拆分

回转装配器 @ 0x1071c4f0：

只认 `REVO` / `NREV` 两种类型（`SREV` 走别处）：

```c
ang   = getReal(ANGL);            // 扫掠角
sweep = fabs(ang);
axisAngle = 0.0;  origin = (0, 0);  start = 0.0;
if (sweep >= 1e-6) { if (ang < 0) axisAngle = 180.0; }   // 负角 = 把轴转 180°，不是反向扫
else                 sweep = 360.0;                       // 角 ≈ 0 视为整圈
```

三条要照抄的约定：**负角靠翻转轴实现而不是负向扫掠**；**角度绝对值小于 1e-6 当整圈处理**；**起始角恒为 0**。

环的处理不走 libgm，而是先过 `G2L_BooleanEngine`（Core3D 自带的 2D 布尔引擎）把外环与洞环解算成若干条闭合 span 链，再逐条 `gm_CreateRevolution(start, sweep, origin, axisAngle, profile)`，多条之间用 `gm_CreateCombination(1)`（UNION，§9.1）组装，最后 `gm_AddCurve` 挂线框。

还有一条**模块相关**的行为：`sweep > 180°` 且 `AVEVA_Module::number()` 为 78（含洞分支）或 78/95（无洞分支）时，把回转**对半拆成 `[0, sweep/2]` 和 `[sweep/2, sweep]` 两个 `GM_Revolution` 再 UNION**。不满足条件时就是一个整的回转体。这不是几何需要，是特定模块下游的兼容处理——**我们不该照抄，但对拍时要知道：同一个 REVO 在不同模块下拿到的树形状不一样，面片数也会差**。

#### `POLYHE` —— 显式点表 + 面表，边可见性逐边给

多面体装配器 @ 0x1071dcf0（`MTR_Entry` 标签 `"create_polyh"`）：

```
id    = gm_CreatePolyhedron()
label = gm_QueryLabel(id)

第一遍：POLPTL 子元素（点表）→ 逐个 PAVERT 读 POS 存成点表（按 DB_Ref 索引）
第二遍：POLFAC 子元素（面）：
    facet = gm_AddFacetToPolyhedron(label, id)
    每个 LOOPTS 环：reverse = getBool(LMIRR)          // 环方向取反标志
            refs    = VXREF[]                         // 顶点引用数组（ELEMENT[500]）
            bits    = INVI[]                          // 每边一位的不可见标志（BOOL[500]）
            按 reverse 决定正序还是倒序遍历 refs：
                顶点首次出现 → iV = gm_AddVertexToPolyhedron(point, id)，记进 map 去重
                否则复用已有 iV
                edgeType = INVI[i] ? 1 : 2            // 1 = Invisible(平滑)，2 = Visible(硬)，见 §6.4
                gm_AddSideToFacetOfPolyhedron(iVprev, iV, edgeType, facet, id)
            收尾再补一条 last → first 的边
校验：gm_ValidateObject(id) 必须 == 1，否则 gm_RemoveObject 并按 −79…−75 映射到消息号 23…27
```

四个值得记的点：

1. **顶点去重是按 DB_Ref 做的，不是按坐标。** 同一个数据库顶点被多个面引用时复用同一个 libgm 顶点下标；坐标相同但来自不同 DB 元素的两个点**不会**合并。这与 libgm 内部 `vertexAt` 的几何焊接（§6.7）是两套机制，叠在一起用。
2. **边的可见性是逐边显式给的**，来自环元素上的 `INVI` 布尔数组——不是算出来的。这是全文唯一一处"边类型由数据直接指定"的入口，其余图元的边类型都由 `calcFacets` 按曲面语义硬编码。
3. **多边形网格的合法性判据比轮廓严**：`gm_ValidateObject` 要 `== 1`，而 §11.4 的轮廓要 `∈ {4, 5}`。同一个函数的返回值在不同对象类型上含义不同，别混用。
4. **环方向取反是数据里的一个布尔位**，不是靠算面积定向（对照 §11.4 轮廓那边是算叉积定向）。两条路子并存。

### 11.7 ★ dabacon word 码的解码函数

上面那些"数字 word 码"（`TYPE` 取到的类型、`getReal` 的属性号）不是随机 id，是**名字的可逆编码**。仓内 `output/noun_layout.json`（DCHC 字典快照，1935 个 noun / 22092 条属性）里每条属性都带 `hash` 字段，拿它当已知对拍集反推出来的公式是：

```
word(name) = 27^4 + Σ_{i=0}^{L-1} (name[i] − 'A' + 1) · 27^i
           = 531441 + Σ …
```

即 **27 进制、A=1…Z=26、第 0 个字符权重最低、外加固定偏置 531441**，最多 6 个字符。反解就是减掉 531441 再逐位除 27。

对拍结果：字典里 22092 条纯字母属性名中 21171 条精确命中；216 个不同的落空名字要么**超过 6 个字符**（word 装不下，另走一套），要么是显示名与底层 word 不同的别名（如 `ORIF` 的 word 实为 `ORIL`）。所以这个函数**对 ≤ 6 字符的规范 word 是准确的**，用在超长名或别名上不成立。

这条不只服务本节——**任何在 core / Core3D 里遇到的裸整数 word 码都能就地读出名字**，不必再回字典查表。本节用它解出来的全部码：

| 码 | 名字 | 用途 |
|---|---|---|
| `642215` | `TYPE` | 取元素类型（`sub_1068EBC0` 的唯一实参） |
| `773119` | `ANGL` | `REVO` 扫掠角 |
| `675926` | `HEIG` | `EXTR` 挤出高 |
| `545713` | `POS` | `PAVERT` 顶点坐标 |
| `3832294` | `VXREF` | `LOOPTS` 的顶点引用数组 |
| `725013` | `INVI` | `LOOPTS` 的逐边不可见标志 |
| `10458597` | `LMIRR` | 环方向取反标志 |
| `183671242` / `44236870` | `POLPTL` / `POLFAC` | 多面体的点表 / 面 |
| `857721` / `837964` / `837961` | `LOOP` / `SLOO` / `PLOO` | 三种环 |
| `840259` | `SPRO` | 标准截面（走解析路径） |
| `900968` `900977` `942751` `1008005` | `EXTR` `NXTR` `SEXT` `NSEX` | 挤出族 |
| `842877` `968612` `968617` `643505` | `REVO` `NREV` `SREV` `NSRE` | 回转族 |
| `640105` | `PANE` | 板 |
| `535968` | `REF` | 引用 |

### 11.8 2.10 与 3.1 的差异核对

前十节最初是在 2.10 版 libgm / libgeom 上拆的。拿到 3.1 版之后逐条复核，**没有发现任何一条结论需要改**。

体量变了不少：libgm 从 780 KB / 2447 个函数长到 1.13 MB / 9006 个函数，libgeom 从 368 KB 到 499 KB；代码生成也从 x87 换成了 SSE2。但**几何语义一条没动**：

| 复核项 | 2.10 | 3.1 | 结论 |
|---|---|---|---|
| 四个全局容差 `arctol_` / `normtol_` / `tangtol_` / `restol_` | `0.1` / `1e-6` / `5.0` / `0.1` | 同 | 一致 |
| `d2_numberOfSegmentsForCircle`：45° 上限、`ceil(360/step)`、取整到 4 的倍数 | ✔ | ✔ | 一致（3.1 把 `2·(180/π)` 折进常量 `114.59155902616465`） |
| `gm_CreateCircularTorus` 存内外半径；`nRing` 用 `rOutside`、`nProfile` 用管半径 | ✔ | ✔ | 一致 |
| `gm_CreateSlopeEndedCylinder` 的 `a5,a6` 先于 `a3,a4` 落位 | 字段赋值 | ctor 形参 `(a1,a2,a5,a6,a3,a4)` | 一致，3.1 的 ctor 签名把这个错位写在明面上 |
| `GM_EDish::knuckleRadiusToUse` 公式 | ✔ | ✔ | 一致 |
| `doFacetCancellation` 的 **175°** 反向面对消门槛 | ✔ | ✔（`rad·57.29577951308232 >= 175.0`） | 一致 |
| `isTangentDiscontinuity` 的 22.5° 常数与门槛 `2·sin/√(cos⁴+sin²)` | ✔ | `cos=0.9238795325112867`、`sin=0.3826834323650898`、门槛 `0.8182115951456252` | 一致，**并且独立坐实了 §6.8.3 那个曾被写错成 `cos²` 的分母** |

一处**接口签名变化**：`GM_Facets::doFacetCancellation` 从 2.10 的 `(double)` 变成 3.1 的 `(double, unsigned int, unsigned int)`，多出两个整型参数（疑似处理范围的起止下标，未拆）。**门槛值本身没变**，§6.8.2 的算法描述仍然成立。

---

## 十二、对 gen-model 的启示

1. **分段数应当由容差算，不是常数。** `src/fast_model/mesh_primitives.rs` 的 `DEFAULT_CIRCULAR_SEGMENTS = 36` 只在 `r ≈ 25 mm`、`tol = 0.1 mm` 时与 AVEVA 一致；`r = 500 mm` 的储罐 AVEVA 会给 160 段，我们只有 36，弦高误差约 `500·(1−cos(5°)) ≈ 1.9 mm`。
2. **`sweep_mesh.rs::arc_segments` 的公式是对的，但三处细节与 AVEVA 不同**，会造成对拍不齐：
   - 我们 `clamp(3, 512)`，AVEVA 是**最少 8**（45° 上限）、**最多 1000**；
   - 我们没有**向上取整到 4 的倍数**，导致同半径的圆柱与包围盒/相邻件顶点不对齐；
   - 我们对部分弧直接 `ceil(angle / max_step)`，AVEVA 是 `ceil(sweep / (360/nFull))`，即先算整圆再切——同半径的整圈与弧段角度栅格一致。
   要与 AVEVA 视觉对齐，这三条都要照抄。
3. **`sqrt(2t)` 那条小角度分支只在封头上，分段数公式里没有。** 这两处以前被混为一谈，实际是两个不同的量：
   - `GM_SDish` / `GM_EDish` 的**半张角**用 `t = height/R`，`t ≤ 1e-6` 时走 `√(2t)`（§5.6）；
   - `d2_numberOfSegmentsForCircle` 的**弦高判据**用 `t = tol/r`，两版 libgeom 都是老老实实 `acos(max(1−t, 0))`，**没有**小角度分支（§4.2，已在 2.10 与 3.1 两版反汇编上分别确认）。
   所以我们 `(1.0 - chord_tol / radius).clamp(-1.0, 1.0).acos()` 的写法**与 AVEVA 一致，不要"改进"它**——加了 `√(2t)` 反而会在大半径上算出与 AVEVA 不同的段数。真要防浮点抵消，只该防在封头半张角那一处。
4. **布尔之后我们丢了法线，libgm 没丢。** `mesh_primitives.rs` 用"硬边处复制顶点 + 解析法线"达到了与 libgm `GM_EdgeType` 分组等价的效果（端盖有自己的一份顶点带平面法线，侧面带曲面法线）——这条是对齐的。但 `manifold_csg.rs::…` 从 Manifold 取回网格时 `normals: vec![]`，**CSG 之后法线全部丢失**。libgm 走的是 `GM_Facets → AM_Body → AM_CoEdge::calcStartNormal`，布尔产生的新边带 `GM_EdgeType 5/6`（不可见曲面边），因此切出来的截面仍能正确参与平滑分组。我们要么在布尔后重建"按硬边分组"的法线，要么把 libgm 的边类型语义搬进来。
5. **超限要有回执，不能只打日志。** libgm 的 `n > 1000 → 截断 + gm_message(1001)` 是典型的静默降级：调用方拿不到任何返回码。我们如果要做等价的容差上限，必须把它做成批次回执里能看见的一条，符合 AGENTS.md 「静默失效是最高级别缺陷」。
6. **`Snout` 的偏移是对半劈的 —— 这里我们和 AVEVA 差了半个偏移。** libgm：底心 `(−xoff/2, −yoff/2, −h/2)`、顶心 `(+xoff/2, +yoff/2, +h/2)`。`mesh_primitives.rs::gen_snout`：底心 `(0, 0, −h/2)`、顶心 `(xoff, yoff, +h/2)`。两份网格相差一个 `(xoff/2, yoff/2, 0)` 的平移；除非调用方另外补偿，偏心大小头在 AVEVA 里会偏到另一个位置。同理 `Block` 在 libgm 是**体心**在原点，不是底心。
7. **CSG 前先比包围盒**：`gm_combine` 第一件事是 `limits.intersects()`，不相交就退化成 append。我们的 `manifold_bool` 路径应保留同样的早退。
8. **id 单调不复用 + 集中 idMap + 每个入口 `checkIdAndWarn`**：这套模式与我们的水位/句柄治理同构，可以直接借鉴到 `pdms_inst` 的实例句柄上。
9. **要与 AVEVA 的线框/消隐对齐，必须保留"边可见性"这一位。** `AM_SGL` 索引流里顶点下标的符号就是它（§7.1）。我们的 `PlantMesh` 只有 `vertices / normals / indices / wire_vertices`，没有逐角点的边标志；`wire_vertices` 是另建的一套。若将来要出 DRAFT 风格的线框，这一位得从图元生成阶段一路带下来。
10. **带洞平面不必先三角化。** libgm 的 `formation data` 用 `+n` / `−n` 表达外环与洞，把三角化推迟到消费端（sgl5NET）。我们现在在 `manifold_csg` 里直接落三角，好处是简单，代价是丢了环结构、也就丢了"哪条边是洞的边界"。
11. **布尔的里外判据可以只用一个带符号整数**（`retain = keep + sense · containment`，§9.4）。我们如果自己写面片布尔的兜底路径，这个编码值得照抄——它把三种运算和"要不要翻面"压进同一条表达式，分支少、不容易漏。
12. **后布尔清理有三把刀**：`doEdgeCracking`（边打断）→ `doFacetCancellation`（反向面对消，175° 门槛）→ `normaliseStage2`（交线边硬/软定型，22.5° 判据）。`manifold_csg.rs` 走 Manifold 库做布尔时这三步全部缺失——Manifold 自己有三角化级的清理，但没有"面片边类型"这一层语义。如果我们想在布尔后恢复正确的平滑着色和硬边，至少要补 `normaliseStage2` 等效的逻辑：遍历布尔切出的新边，按两侧面法线偏差判定硬/软。
13. **边被打断时两端映射数必须一致**。`addRetainedEdges` 在两端映射数不等时直接抛 `GM_SetOpException`，不做兜底。这是拓扑正确性的强断言——如果我们的面片布尔实现也有类似的"边上中间点"概念，应该照搬这个断言。
14. **`mesh_primitives.rs` 的形参顺序与 AVEVA 是对的，可以放心。** `gen_snout(r_bottom, r_top, height, x_offset, y_offset)`、`gen_slope_ended_cylinder(radius, height, btm_angles, top_angles)`、`gen_pyramid(xbot, ybot, xtop, ytop, height, xoff, yoff)` 三个签名与 §11.3 逐位对齐（底在顶前、剪切角底在顶前）。但**读 `GM_SlopeEndCyl` 的字段时顺序是反的**（`+7/+8` 是顶），若将来要从 libgm 侧反向读参数别踩这个。
15. **环体的环向分段要按 `rOutside` 算，不是中心线半径。** libgm 的 `nRing = numberOfSegmentsForPartRev(rOutside, …)`（§5.8）。取外半径是因为弧上离轴最远的那条母线弦高最大，用中心线半径会低估分段数、外缘出现可见棱。我们若按 `(rIn+rOut)/2` 算，`DN500` 弯头这类 `rOut/rCentre` 比值大的件会明显偏少。
16. **`DISH` 的三分支要照抄，尤其两个反直觉分支**（§11.3）：`HEIG <= 0` 造 1 mm 厚圆片而不是报错；`RADI` 只当布尔开关，真正的转角半径由 `R`、`H` 现算。如果我们直接把目录里的 `RADI` 当转角半径喂给几何，形状会和 AVEVA 不一致——而且是"看着挺像、量起来不对"的那种不一致。
17. **`RINS` 钳非负、`SLCY` 四角归一化到 (−90°, 90°]，是入口清洗不是几何逻辑。** 这两条在 Core3D 侧做，libgm 侧不做。我们的图元入口若直接吃目录值，脏数据会一路带到三角化（`SLCY` 传 100° 会让 libgm 直接返回空面片集，见 §5.4）。这类清洗应集中在一处，别散落在各 `gen_*` 里。
18. **轮廓构建的六道门值得整套照搬**（§11.4）：定向归一化到逆时针、质心平移后再建、圆角用切点+bulge 两段表达、相对容差 `maxCoord·1e-5`、末点吸附到首点再闭合、`|area| < 1e-6` 判空。其中"相对容差"这一条最容易漏——我们现在多处用绝对 `1e-6`，对厂区级坐标（1e5 mm 量级）来说过严。
19. **`gm_SetDefaultLabel` 是共享全局态。** Core3D 在每次建图元前设一次。我们如果把几何生成并行化到多线程又复用类似的"默认标签/默认容差"全局量，会串味。libgm 的 `GM_User::arctol_` 等四个容差同理（§4.1）。
20. **带洞挤出应当是"多环轮廓"而不是"挤出后再布尔减"。** AVEVA 走 `gm_CreateExtrusionGroup`（§11.6），洞是轮廓层的内环，从头到尾没进过 CSG。我们若用布尔减洞，除了多花一次布尔，还会丢掉"哪条边是洞的边界"这一层语义（对照启示 10），而且会踩上启示 12 说的后布尔清理缺失问题。
21. **`REVO` 的角度有三条约定，不照抄就对不齐**（§11.6）：负角是**把轴转 180°**而不是反向扫掠；`|角| < 1e-6` 当整圈；起始角恒为 0。这三条都在 Core3D 侧，libgm 只收结果。
22. **同一个 `REVO` 在不同 AVEVA 模块下会生成不同的树。** `sweep > 180°` 且模块号为 78 / 95 时会对半拆成两个回转再 UNION（§11.6）。对拍面片数或树结构时，这是一个必须先问清楚"对方在哪个模块导出的"的变量——不是我们的实现错了。
23. **`POLYHE` 的边可见性是数据给的，不是算出来的**（§11.6）：面元素上有一张逐边位图，`1 → Invisible(平滑)`、`0 → Visible(硬)`。我们解析多面体时如果按几何夹角自己判硬软，会和 AVEVA 不一致——这类元素的作者是有意指定过的。同理它的顶点去重按 `DB_Ref` 而非坐标，坐标重合但来源不同的点不合并。

---

## 十三、未验证 / 待办

- `gm_QueryFacetDataSize` 与 `gm_QueryFacetData` 的量纲差异只是静态观察，未跑实例确认。
- `retain` 为负时面片如何翻向：从 `appendSurfaceDataFrom(negate)` 推出 B 侧曲面法线取反，但 `addRetainedFacets` 里面片本身不做翻向——**翻向只在曲面层**，面片的拓扑朝向不变。这个推论未在边一级坐实。
- `HL_Picture` 的遮挡消解（`obscure` / `obscureFaces` / `selfHide`）只做了功能归类，算法本身未拆；本次只拆了空间细分和 GM_Verge 部分。
- `GM_Facets::doFacetCancellation` 在 3.1 多出的两个 `unsigned int` 参数是什么没拆（§11.8）。门槛值已确认未变，但这两个参数可能限定了对消的处理范围。
- §11.8 的复核是**抽查关键常量与关键函数**，不是全量 diff。3.1 多出的 6559 个函数没有逐一过；两版是否有新增的清理步骤或新图元类型未系统排查。
- `CSG_BasicRUL` 没出现在 `CSG_PrimitiveUtilities::initialise` 的注册表里，挂接点未找到（两版皆然）。
- 3.1 里逐角圆角求解那个函数没定位（2.10 是 `0x10690fc0`）；§11.4 第 4 步的描述来自 2.10。
- `G2L_BooleanEngine`（`REVO` 用来解算外环减洞环的 2D 布尔引擎）只认出了接口（`reset` / `outputObject` / `outputKcurve` / `outputSpan`），算法未拆。
- `AVEVA_Module::number()` 的 78 / 95 具体是哪两个模块没查（§11.6 的对半拆分条件）。
- §11.7 的 word 函数对**超过 6 个字符**的名字不成立，对少数"显示名 ≠ 底层 word"的别名也不成立（字典里 216 个这类名字）。用它反解长名会得到错的字符串。
- §11.3 / §11.4 / §11.6 全部是静态反编译结论，**未与运行时对拍**：没有跑过 E3D 造一个已知尺寸的 `DISH` / `SLCY` / `REVO` 再回读 `gm_QueryItem` 验证。`DISH` 的 `RADI` 不入参这一条尤其值得实测一次，因为它会直接改我们的目录解析。

### 第四轮补齐（Core3D 上游）

- **noun → `gm_Create*` 分派全表**：`CSG_PrimitiveUtilities::initialise` 的 24 个 noun 注册 + `CSG_BasicPrimitive::findPrimitive` 查表 → §11.1 / §11.2。这条是 `teach/MISSION.md` 课 03 遗留第 3 条的正主，至此闭合（数值码除外，见上）。
- **逐图元属性 → 参数对照**：13 个 `CSG_BasicXXX::getPrimGeom` 的读取顺序与算术（直径减半、`RINS` 钳非负、`SLCY` 四角归一化）→ §11.3。
- **`DISH` 三分支**：`HEIG <= 0` → 1 单位厚圆片；`RADI` 只作布尔，转角半径由 `R`/`H` 现算 → §11.3 / §5.7。
- **`LOOP`/`VERT` → `D2_Profile` 的六道门**：逆时针归一化、质心平移、圆角切点+bulge、相对容差 `maxCoord·1e-5`、末点吸附闭合、`gm_ValidateObject ∈ {4,5}` → §11.4。
- **摆放与开洞流程**：`hardType` 取 noun、`gm_SetDefaultLabel` 先行、`TMPL`/`FIXING` 模板孔、`BPANEL` 清变换 → §11.5。
- **`EXTR` / `REVO` / `POLYHE` 三个带子元素的图元**：带洞挤出走 `gm_CreateExtrusionGroup` 而非布尔减；`REVO` 的负角翻轴 / 零角整圈 / 起始角恒 0 三约定，以及模块 78·95 下 `sweep > 180°` 的对半 UNION；`POLYHE` 的 `POLPTL`/`POLFAC`/`LOOPTS` 三层结构、`INVI` 逐边可见性、按 `DB_Ref` 去重 → §11.6。
- **dabacon word 码的解码函数** `word(name) = 531441 + Σ (name[i] − 'A' + 1)·27^i`，用仓内 `output/noun_layout.json` 的 22092 条属性对拍得出 → §11.7。本节全部裸整数（`TYPE` / `ANGL` / `POLPTL` / `LOOPTS` / `INVI` …）据此解出，不再有"未解析的码"。这条可复用到任何在 core / Core3D 里遇到 word 码的场合。
- **版本口径改为 3.1**：§十一 全部结论已在 3.1 版 Core3D 上重新验证并逐条与 2.10 对拍。图元侧完全一致；差异只有 3.1 多挂 10 个 `CSG_TreeBuilderCat` 的目录 noun。附录给出两版地址。
- **libgm / libgeom 也拿到了 3.1 版并复核**：容差、分段数公式、环体半径语义、`SLCY` 参数错位、封头转角半径、175° 对消门槛、22.5° 相切判据全部两版一致 → §11.8。其中 22.5° 那条**独立坐实了上一轮对 §6.8.3 分母写错的订正**。唯一的接口变化是 `doFacetCancellation` 多了两个整型参数。
- **订正启示 3**：`√(2t)` 小角度分支只存在于封头半张角，分段数公式里两版都没有。原先那条建议会让我们主动偏离 AVEVA。
- **两处签名订正**（由上游坐实，libgm 单侧读不出来）：`gm_CreateCircularTorus` 头两参是内外半径而非中心线/管半径（§3.1、§5.8）；`gm_CreateSlopeEndedCylinder` 形参是「底角在前」而字段存储是「顶角在前」（§3.1）。前者顺带订正了 §5.8 那条"分段数交叉使用"的误读，并关掉了上一轮的同名待办。

### 第三轮补齐

- `doEdgeCracking` 完整拆解：O(n²) AABB 粗筛 + `crackEdgesOfFacet`（点近线 / 线近线三种交叉检测）+ `crackEdge`（沿参数排序后逐点拆边）→ §6.8.1。
- `doFacetCancellation` 完整拆解：**175° 反向面对消门槛**（硬编码，5° 容差）+ `cancelFacets`（`enclosureOfLine` 全包围判据）→ §6.8.2。
- `normaliseStage2` 完整拆解：布尔交线边（type 3）→ 硬/软定型。判据 `isTangentDiscontinuity` 用**固定 22.5°** 的几何常数，不用 `tangTol`（5°）—— `tangTol` 只控制"是否执行这个判据"的开关 → §6.8.3。
- `addRetainedEdges` 完整拆解：两端映射数必须相等（否则抛异常）；多映射时沿主轴排序配对 → §9.7。
- `addRetainedFacets` 完整拆解：只做占位壳，真正的边列表由 `buildFacets` 从边的面归属重建 → §9.8。
- `GM_Verge`：轻量边引用 `{owner, iV1, iV2}`，只检查 XY 不检查 Z（屏幕空间消隐），用于 `HL_SceneElement` → §十。

### 第二轮已补齐

- `AM_SGL` 的 `formation data` 编码 → §7.1（±环长 + `(法线索引, ±顶点索引)` 对，1-based，符号 = 边可见性）。
- `GM_EdgeType` 九个取值的**官方名字** → §6.4（来自 `GM_Edge::printOn`）。
- `GM_CutType` = `0 GM_CUTALL` / `1 GM_ONLYSOLIDS` → §9.1。
- `GM_Facets` 内容类别 = wireframe / sheet / solid → §6.5。
- 布尔的里外判据 `retain = keep + sense·containment`，以及 `clashVtSo3D` 的 +Z 射线计数 → §9.4 / §9.5。
- 五个 SetOp 阶段的**真名**（`addRetainedVertices` / `addRetainedEdges` / `addRetainedFacets` / `addIntCurveVertices` / `addIntCurveEdges`），取自 `FL_Monitor` 标签。
- `GM_Collar` = `gm_CreateRuledSolid`，`GM_SweptSolid` 是外轮廓 + N 个内轮廓的基类 → §5.14。
- `HL_Cell` 的 kd-tree 分割打分式与深度上限 20 → §十。

---

## 附录 · 关键地址索引（libgm.dll，ImageBase 0x10000000）

| 地址 | 符号 |
|---|---|
| `0x10009160` | `GM_Item::GM_Item(double)` |
| `0x1004a750` / `0x1004a7c0` | `GM_IdMap::add` / `get` |
| `0x100353f0` | `GM_Facets::GM_Facets(nFacets,nEdges,nVertices)` |
| `0x10035f70` / `0x10036a00` / `0x10036e40` / `0x100370a0` | `addVertex` / `addEdge` / `addFacet` / `addSurface` |
| `0x100371b0` | `GM_Facets::addFacetAndFlatSurface` |
| `0x10040e70` | `GM_Facets::vertexAt`（焊接） |
| `0x10040920` / `0x100409d0` / `0x10045400` | `normalise` / `Stage1` / `Stage2` |
| `0x10041230` | `GM_Facets::doEdgeCracking` |
| `0x100417b0` | `GM_Facets::crackEdgesOfFacet` |
| `0x10042210` | `GM_Facets::crackEdge` |
| `0x100429b0` | `GM_Facets::doFacetCancellation` |
| `0x10043ce0` | `GM_Facets::cancelFacets` |
| `0x10044c40` | `GM_Facets::enclosureOfLine` |
| `0x10044f60` | `isTangentDiscontinuity`（22.5° 固定判据） |
| `0x10043500` / `0x10043a80` / `0x10041100` | `separateInsideOutPart` / `mergeNeighbour` / `resolveFacet` |
| `0x10070040` | `GM_Verge::GM_Verge(GM_Facets*, int, int)` |
| `0x10070060` | `GM_Verge::outsideRect(D3_Limits)` |
| `0x10040350` / `0x1002ce10` | `GM_Facets::sectionBy` / `GM_CompFacets::sectionBy` |
| **`0x10064140`** | **`gm_combine`（布尔核，`GM_SetOp.cxx`）** |
| `0x10063200` / `0x10063400` / `0x10063cc0` | `addRetainedVertices` / `addRetainedEdges` / `addRetainedFacets` |
| `0x10063f30` / `0x10064050` | `addIntCurveVertices` / `addIntCurveEdges` |
| `0x1004d7e0` / `0x1004e260` | `clashVtSo3D`（点在实体内，A 侧 / B 侧） |
| `0x1004d200` / `0x1004d630` | 单面片的带符号穿越计数 + 交点 Z 插值 |
| `0x1004bcf0` | `GM_IntCurve::create`（面-面求交） |
| `0x10030870` | `GM_Edge::printOn`（`GM_EdgeType` 名字表） |
| `0x100630a0` | `GM_Section::printOn`（`GM_CutType` 名字表） |
| `0x1001e2b0` / `0x100299e0` / `0x1002b3c0` | `gm_CreateRuledSolid` / `GM_Collar::calcFacetsWithoutSurfaces` / `setSpanSteps` |
| `0x10071c50` / `0x10072110` | `HL_Cell::split` / `chooseSplitLine` |
| `0x10068170` | `GM_SolidCombination::calcFacets` |
| `0x100056e0` / `0x1000d6e0` | `AM_Body::AM_Body(GM_Facets)` / `AM_SGL::AM_SGL(AM_Body)` |
| `0x1000c390` | `AM_CoEdge::calcStartNormal`（平滑组） |
| `0x1000a6f0` / `0x1000a830` | `gm_QueryFacetDataSize` / `gm_QueryFacetData` |
| `0x1005ae80` / `0x1005afa0` / `0x1005c520` | `GM_ProfileTessellator` ctor / run / dtor（GLU） |
| `0x100af180` / `188` / `190` / `198` | `GM_User::arctol_ / normtol_ / tangtol_ / restol_` |
| `0x1001d310` / `0x1001cde0` | `gm_CreateCircularTorus` / `gm_CreateSlopeEndedCylinder`（字段落位见 §3.1） |
| 图元 `calcFacets` | Block `0x10013ca0`、Cylinder `0x1002f830`、Snout `0x10066510`、SlopeEndCyl `0x10065da0`、Sphere `0x10068d30`、SDish `0x10062170`、EDish `0x10030ca0`、CircTorus `0x10028760`、RectTorus `0x1005e2f0`、Pyramid `0x1005cd50`、Extrusion `0x10032ec0`、ExtrusionGroup `0x100342a0`、Revolution `0x10060260` |

**libgeom.dll**（ImageBase 0x10000000）：`d2_numberOfSegmentsForCircle` 2.10 `0x1001d550` / 3.1 `0x1002ba70`、`d2_numberOfSegmentsForPartRev` 2.10 `0x1001d5f0` / 3.1 `0x1002bb20`、`D2_Span::getApproxPolyLine` 2.10 `0x1001be00`、`getApproxPolyLineInSteps` 2.10 `0x1001be80`。

**libgm.dll 3.1** —— §11.8 复核用到的地址（上表是 2.10 口径）：`gm_CreateBox` `0x10038d20`、`gm_CreateSnout` `0x100392a0`、`gm_CreateSlopeEndedCylinder` `0x100394b0`、`gm_CreateCircularTorus` `0x10039a00`、`gm_CreateEllipticalDish` `0x10039e90`、`GM_CircTorus::calcFacetsWithoutSurfaces` `0x10047150`、`GM_Facets::doEdgeCracking` `0x10064050`、`doFacetCancellation` `0x100652d0`、`normaliseStage2` `0x10066e70`、`isTangentDiscontinuity` `0x10066a40`、`GM_EDish::knuckleRadiusToUse` `0x100556a0`、`GM_Edge::printOn` `0x10054300`、`AM_CoEdge::calcStartNormal` `0x1001e100`、`GM_User::normtol_/arctol_/tangtol_/restol_` `0x10109020` / `028` / `030` / `038`。

**Core3D.dll**（ImageBase 0x10000000）—— §十一 用到的地址。**3.1 是本节的基准版本**，2.10 一列供与 2.10 版 libgm 对照时用；两版内容已逐条对拍一致（§十一 开头）。

| 符号 | 3.1 | 2.10 |
|---|---|---|
| `CSG_PrimitiveUtilities::initialise`（noun 注册表全量） | `0x10727540` | `0x106a9f10` |
| `CSG_BasicPrimitive::findPrimitive` / `found` | `0x107266f0` / `0x10726730` | `0x106a95d0` / `0x106a95f0` |
| `CSG_TreeBuilderPrimitive::getCSGTree` | `0x10726890` | `0x106a9c80` |
| `CSG_TreeBuilderNegativePrimitive::getCSGTree` | `0x10726770` | `0x106a9620` |
| `addStandAloneNegative` | `0x10726620` | `0x106a88a0` |
| `addHolesBelowTemplate` / `addHolesBelowPrimitive` | `0x107263a0` / `0x10726150` | `0x106a9750` / `0x106a9a70` |
| `DB_Create_D2_Profile` 装配（§11.4） | `0x1071a2e0` | `0x106926f0` |
| `gm_CreateProfile` 包装（建 + 验 + 删） | `0x1071e7d0` | `0x10694770` |
| 逐角圆角求解（切点 + bulge） | 未定位 | `0x10690fc0` |
| `EXTR` 多环装配（`gm_CreateExtrusionGroup`） | `0x1071c0e0` | `0x10695a00` |
| `REVO` 装配 | `0x1071c4f0` | `0x10696fd0` |
| `POLYHE` 装配（`create_polyh`） | `0x1071dcf0` | `0x106979f0` |

`CSG_Basic*::getPrimGeom`：

| | BOX | CYL | CON | PYR | DIS | SNO | CTO |
|---|---|---|---|---|---|---|---|
| 3.1 | `0x10726a90` | `0x10726ca0` | `0x10726b30` | `0x10726f90` | `0x10726d10` | `0x10727450` | `0x10726be0` |
| 2.10 | `0x106a8970` | `0x106a8a00` | `0x106a8a70` | `0x106a8b10` | `0x106a8c20` | `0x106a8d30` | `0x106a8e10` |

| | RTO | POL | SLC | EXT | REV | RUL |
|---|---|---|---|---|---|---|
| 3.1 | `0x10727140` | `0x10726f10` | `0x107272d0` | `0x10726e50` | `0x107270c0` | `0x10727220` |
| 2.10 | `0x106a8ec0` | `0x106a8f90` | `0x106a9010` | `0x106a9190` | `0x106a9260` | `0x106a92e0` |
