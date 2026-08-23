# Implementation Plan：按 libgm 原语表用 manifold-csg 替换 OCC

**规格**：`specs/009-retire-occ/spec.md`
**决策**：ADR-030（含 2026-08-15「IDA 修订」与 2026-08-23「IDA 修订二」）、ADR-044
（曲面段数与单位网格身份）、ADR-026、ADR-029
**IDA**：`D:\AVEVA\Everything3D3.1\Core3D.dll.i64`（`idalib-32268`）、
`libgm.dll.i64`（`idalib-18608`）、`libgeom.dll.i64`（`idalib-21956`）
**逆向全文**：`../plant-4/libgm-boolean-algorithm.md`（相对本仓根）
**内核权威**：libgm `gm_Create*`，不是 OCC BRep。全程只用本地 `manifold-csg`。

> **2026-08-23 重写。** 上一版把「覆盖面」当主线。覆盖面已经做完了，主线换了：
> 剩下的全部是**口径**与**验收**。三个曾经写作「有意的 OCC 回退」的分支经 IDA 核对
> 都不成立（ADR-030 修订二），删掉它们并不需要新几何；真正没做的是段数对齐、回转轮廓
> 的第二套离散口径、以及一条都没跑过的 RVM 门。

## Constitution Check

- **水位承诺**：本计划不改水位 / 队列 / 暂存窗口。ADR-044 的身份键变更走整库重建
  （ADR-021 的文件回退同一条路），水位随重建归零再建立，不做就地改写。
- **单一规则**：三角化只经 `tessellate_libgm_param`；段数只经 `libgm_discretise`；
  布尔只经 manifold-csg。挤出与回转两套轮廓离散**各一个入口**，不得合并（ADR-044 决策 3）。
- **响亮失败**：`tessellate_libgm_param` 收口后不再有「回退 OCC」语义的 `Ok(None)`；
  空轮廓 / 空网格 / 表达不出的输入一律 `bail!`。
- **队列收口**：不新增队列 action。
- **标识真值**：不把 geo_hash 当 dbnum。单位网格身份按 ADR-044 改键，改键与
  `canonical_unit_param_json` 的共享行收口同批落地。
- **可执行守护**：每个工作包有纯函数单测；曲面段数有对照表单测；扫掠与目录件有 RVM 门。

**Complexity Tracking**：
Phase 4 之前 `occ` 仍在 default，但它已经**不再承担任何形状**——保留它只为
`mesh_compare` 的参照侧与 `loop_model` 的一条历史断言（WP-H 收口）。
aios-core `gen_model` 不再捆绑 `manifold-sys`（与 manifold-csg 链接冲突）；该改动目前
靠本地 vendor patch，推 main 前必须先合进 aios-core 再升 rev。

## 现状真值表（2026-08-24 逐条核对源码 + IDA）

`tessellate_libgm_param` 已是穷举 match，16 个变体全部有分支。**没有一个变体因为
「建不出来」而回退 OCC**（WP-F 收口，`None` 只剩「非形状」一个含义）。

「参数」一列 2026-08-24 新增：`gm_Create*` 的**实参顺序**是否经 IDA 核实。此前
T011 / T013 / T016 一直空着，现已全部两头对照过（libgm 构造函数字段序 × Core3D
`CSG_Basic???::getPrimGeom` 调用点），**没有一条顺序是错的**——但顺带查出两处
**摆位**错和一处**形状族**错，见表内加粗那三处（SSCL / `PrimLSnout` / 椭圆碟）。

| `PdmsGeoParam` | 生成 | 段数口径 | 参数 | 欠项 |
|---|---|---|---|---|
| `PrimBox` | 单位箱 | 不涉及 | 不涉及 | — |
| `PrimPolyhedron` | 逐面 earcut | 不涉及 | 不涉及 | 活库 0 实例，未经现场验证（T045a） |
| `PrimPyramid` / `PrimLPyramid` | `gen_pyramid` | 不涉及 | ✅ T016 | 偏移半分与全宽约定均已对上 |
| `PrimExtrusion` | `tessellate_extrusion` / `tessellate_arc_wall` | ✅ 挤出口径 | 不涉及 | — |
| `PrimRevolution` | `tessellate_revolution` | ✅ 配对口径（T040） | 不涉及 | 出平面轴 `bail!`；轴心吸附已补（T035） |
| `PrimLoft` | `sweep_mesh::sweep_solid_mesh` | ✅ 绝对容差（T042） | 不涉及 | 四道 RVM 门一条没跑（WP-J） |
| `PrimLCylinder` / 无切角 `PrimSCylinder` | 单位柱 | ⛔ 具名欠账 32 | 不涉及 | 身份键挡着，等 T041 |
| 切角 `PrimSCylinder`（SSCL） | `gen_slope_ended_cylinder` | ✅ `circle(radius)` | ✅ T016 | **剪切角未折进 (−90, 90]**（Core3D 会折）；活库 0 实例 |
| `PrimSphere` | `unit_sphere()` | ⛔ 具名欠账 16×36 | 不涉及 | 身份键挡着，等 T041；活库 0 实例 |
| `PrimLSnout` | `gen_snout` | ✅ `fmax(rBtm, rTop)` | ✅ T011 | **偏心摆位错（T050）**、**YOFF 未接（T051）** |
| `PrimDish`（球碟） | `gen_spherical_dish` | ✅ 两方向均权威 | ✅ `CSG_BasicDIS` `0x10726D10` | — |
| `PrimDish`（椭圆碟） | `gen_elliptical_dish` | ✅ 三方向均权威 | ✅ | 2026-08-24 换成托里球形封头（T038a）；活库 0 样本，未经现场验证 |
| `PrimCTorus` | `gen_circular_torus` | ✅ `partRev(rOut)` + 管截面 | ✅ T013 | `RINS` 未按 Core3D 夹 `fmax(·, 0)` |
| `PrimRTorus` | `gen_rectangular_torus` | ✅ `partRev(rOut)` | ✅ T013 | 同上 |
| `Unknown` / `CompoundShape` | 回 `None` → 标 `bad` | 不涉及 | 不涉及 | — |

「⛔ 具名欠账」不是「忘了改」：段数只许有三个出处——权威规则、点名的身份键欠账
（`unit_mesh_identity`）、点名的口径欠账，`every_segment_count_is_named_or_computed`
逐行扫着。

> **「段数口径」那一列的 ✅ 说的是「规则对」，不是「现场生效」。** 2026-08-24 实测
> （`docs/evidence/2026-08-24-unit-normalised-curved-primitives.md`）：`PrimDish` /
> `PrimCTorus` / `PrimRTorus` / 不偏心的 `PrimLSnout` 在 `inst_geo` 里**全都是单位几何**
> （`pdia` / `rout` / `pbdm` 恒为 1.0），真实尺寸在实例变换的 `scale` 里。权威规则于是
> 拿到单位半径，`tol/R = 1` 直接撞 45° 下限——**不论多大都是 8 段**。最大那件碟
> （D = 48.9 m）应当 492 段，弦高差出容差的 3,700 倍。挡在前面的仍是身份键：
> **G3 的范围不是「柱与球」，是所有参与复用的曲面原语**，见 T053。

`occ` 在源码里剩下的引用只有四处，全部不在生成路径上：
`src/rvm_baseline/mesh_compare.rs` 的 `gen_side`、`src/fast_model/occ_generate.rs` 里两条
`#[cfg]` 测试与已注释的 `apply_insts_boolean_occ` / `apply_cata_neg_boolean_occ`、
`src/fast_model/loop_model.rs` 一条断言、`src/plug_in/water_calculation.rs`（挂在
**Cargo.toml 里根本没定义**的 `opencascade_rs` feature 上，等于常关死代码）。

## 阻塞 Phase 4 的到底是什么

按 ADR-030 决策 1，删 `occ` 的前提是「生产路径上每个会走到的 `PdmsGeoParam` 都能稳定
生成 `PlantMesh` 并通过既有 RVM 对拍门」。前半句已满足，后半句一条都没验。具体三件：

1. **段数口径**（WP-G / ADR-044）。~~曲面原语仍是写死段数。~~ **2026-08-23 起大头已落地**
   （T038 / T039 / T040 / T042）：吃真实尺寸的那几类都按权威规则算了，回转有了自己的
   一套口径，容差收成了一份。剩下两处写死是**具名欠账**而不是遗漏——单位柱与单位球
   卡在身份键上（T041），椭圆碟的经向卡在形状族上（T038a）。`cancelFacets` 只消全等
   重叠（§6.11）的那条论证没变：段数不等于 E3D，共面抵消就整个放弃，这是布尔正确性
   不是画质。
2. **验收能力**（WP-H）。~~`mod gen_side` 整个挂在 `#[cfg(feature = "occ")]` 下。~~
   **解绑本身 2026-08-23 已落地**（T043），`loop_model` 的历史断言与浸水死代码也清了
   （T044）。但解绑只是把尺子从被量的对象上拆下来——**尺子一次都还没量过东西**：
   四道墙体 RVM 门加曲面原语抽检（T046–T049）一条没跑，而 T038a / T041 / T050 三件
   改动全都只能靠它们验收。**现在的瓶颈从「编不过」变成了「没跑过」**，这两件事
   容易混，勾选时要分清。
3. ~~**活库盘点**（WP-I / FR-008）。~~ **2026-08-23 已完成**，见 WP-I：三个「假回退」
   实测全为 0 行，G3 的爆炸半径最坏 37 份 `.mesh`，而写死的 32 段只有 2.0% 的圆柱实例
   是对的。盘点不再阻塞，剩下的两件才是。

---

## WP-F  假回退收口（依据 ADR-030 修订二）

三条分支各有独立的正确归宿，不是同一个改法。做完之后 `tessellate_libgm_param` 的返回
类型里 `None` 不再表示「回退 OCC」。

### F1 出平面回转轴 → `bail!`

- **证据**：`GM_Revolution::GM_Revolution`（libgm 3.1 `0x10033830`）签名是
  `(double startAngle, double finishAngle, D2_Point const& origin, double axisAngle,
  GM_Profile*, double tol)`——轴是**平面内的角度**。
  `translatePolygonIntoStandardPosition`（`0x10097810`）把原点搬到零点、
  `rotateBy(90 − axisAngle)` 把轴摆到 +Y。libgm 表达不出出平面轴。
- **本仓侧**：`Revolution` 的唯一构造点是 aios-core
  `../vendor/old-aios-core/src/prim_geo/category.rs`，走 `..Default::default()`，
  `rot_dir` 恒为 `Vec3::X`。
- **改法**：`src/fast_model/manifold_tessellate.rs` 的 `AXIS_IN_PLANE_EPS` 分支从
  `return Ok(None)` 改成 `bail!`，错误信息带上实际的 `rot_dir`。
- **门**：`out_of_plane_revolution_axis_falls_back_to_occ` 改名改断言为「必须报错」；
  加一条源码断言，`tessellate_revolution` 内不得出现 `Ok(None)`。

### F2 轴心吸附（新欠项，不是等价改写）

- **证据**：`movePointsOntoYAxis`（`0x100978A0`）把顶点 x 的绝对值小于
  `GM_User::normtol_` 的顶点 x 精确置 0。
- **改法**：`tessellate_revolution` 在标准位下做同样的吸附。`normtol_` 的取值要另钉
  （**IDA 下一刀**：`GM_User` 的初始化点，与 `arctol_` 同一处）。
- **门**：轮廓贴轴（距离小于阈值）时，回转后轴心不得出现针状面；体积与解析值对拍。

### F3 `CurveType::Spline` → 弧形墙截面，在 manifold 上实现

- **证据**：该变体在整个工作区无构造点，只存在于单测。其 OCC 实现
  `wire::gen_occ_spline_wire` 要求恰好 3 个 SPINE 点，解三点圆后按 `thick` 一半内外偏移，
  拼「外弧 + 直段 + 内弧 + 直段」——环形扇区，不是样条。libgm 侧 `D2_Profile` 的 span
  只带 bulge，`GM_Bezier`（`gm_CreateBezier` `0x10038880`）是靠 `gm_AddCurve`
  （`0x1003A7D0`）挂树的曲线图元，不进实体轮廓。
- **改法**：在 `src/fast_model/manifold_tessellate.rs` 加一支：三点定圆 → 两条 bulge 弧 → 复用
  `libgm_discretise::span_polyline_by_tol` 离散 → 闭环挤出。**不新写弧数学**。
- **门**：环形扇区体积对帕普斯定理（1%）；3 点以外的点数 hard fail；退化成直线
  （三点共线）时 hard fail 而不是给一个空环。
- **注意**：这一支在活库里可能一个实例都没有（WP-I 盘点确认）。即便如此也要实现——
  留着 `Ok(None)` 就等于留一个「关了 `occ` 才会炸」的洞。

### F4 `Unknown` / `CompoundShape` → 直接标 `bad`

- **证据**：两者 `check_valid()` 为 `false`，`gen_occ_shape()` 第一句即
  `Err("Invalid shape")`。回 `None` 只是换个地方报同一个错。
- **改法**：`tessellate_libgm_param` 的返回类型换掉 `Option`，或让这两支返回一个显式的
  「非形状」判定，由 `gen_inst_meshes` 直接推进 `unbuildable`，不经 OCC 分支。
- **门**：源码断言 `src/fast_model/occ_generate.rs` 的 manifold 分支之后不再有
  `#[cfg(feature = "occ")]` 的形状回退路径。

---

## WP-G  离散口径对齐（ADR-044，Phase 4 的硬前置）

### G1 回转轮廓换成 `setNSteps` 口径 —— **规则已钉死（2026-08-23 IDA）**

调用链：`GM_Revolution::calcFacetsWithoutSurfaces`（`0x10097920`）→
`GM_Profile::polygonForFacet`（`0x1008ED80`）→ `GM_Profile::setNSteps(double)`
（`0x1008F2E0`）→ `GM_Profile::getPolygonForFacet`（`0x1008F8B0`）。
`polygonForFacet` 的调用方只有 `GM_Collar` 与 `GM_Revolution`（各含 `validate`）；
**`GM_Extrusion::calcFacets`（`0x10056F10`）不在其中**。

**关键结论：两条路最终调的是同一个格子函数 `D2_Span::getApproxPolyLineInSteps(n)`
（本仓 `span_polyline_in_steps` 已经是它），差别只在喂进去的 `n`。** 所以 G1 不需要新的
弧数学，只需要一套新的段数计算。

#### 1. `pairedSpan(i)`（`0x1008F7F0`）：反向重合边

设 span `i` 是 `pts[i-1] → pts[i]`。
- 起点等于终点（退化）→ 返回 `-1`。
- 否则在 `j = 1..nSpans` 里找**同两点、反方向**的那条：
  `pts[j] == pts[i-1] 且 pts[j-1] == pts[i]`。找到返回 `j`，找不到返回 `-2`。
- 比较是**精确浮点相等**，没有 epsilon（与 §7.4 / §7.6 同一风格）。

即：轮廓上原路折返的那对边互为配对。零厚度翅片、回转轮廓的接缝都会命中。

#### 2. `setNSteps(tol)`（`0x1008F2E0`）：配对取大 + 单调取大

```text
for i in 1..=nSpans:
    r_own  = D2_Span::getRadius(span[i])
    r_pair = pairedSpan(i) > 0 ? D2_Span::getRadius(span[pairedSpan(i)]) : 0.0
    n      = d2_numberOfSegmentsForCircle(fmax(r_own, r_pair), tol)
    nSteps[i] = max(nSteps[i], n)          // 只增不减
```

#### 3. 1000 封顶：**不是逐段截断，是整条轮廓重算**

`polygonForFacet` 在 `setNSteps` 之后取 `getNFacetsRoundProfile()`（`0x1008ECB0`）：

```text
total = 0
for i in 1..=nSpans:
    total += |bulge| >= SPAN_EPS ? trunc(nSteps[i] * |α1 − α0| / 2π) : 1
```

`total > 1000` 时打 `printLimitFacetWarning`，**清空 `nSteps` 数组**（否则单调取大会把
过细的旧值留下），把容差放大成

```text
tol' = tol · ((total − nSpans) / (1000 − nSpans))²
```

再跑一遍 `setNSteps(tol')`。平方是对的：`n ∝ 1/√tol`，所以 `tol` 乘 `k²` 让 `n` 除以 `k`，
恰好把 `total` 拉回 1000 附近。

> **这条修正了既有文档。** `plant-4/libgm-boolean-algorithm.md` §7.9.1 写的是
> 「每个原语都在自己内部 `if (n > 1000) n = 1000`」，`libgm_discretise` 的模块文档写的是
> 「截面那条路上没有封顶」。对曲面原语两句都对；对**轮廓**这条路两句都不对——
> 封顶存在，但形式是全局容差重标定，会改变**每一段**的段数，而不是只削掉超限的那些。
> §7.9.1 需要补一条修订（见 tasks 的 T040a）。

#### 4. 顺带发现：平滑/硬边标记也在这条路上

`getPolygonForFacet` 的第二个出参（`FL_vector<int>`）逐顶点计数，并在
`D2_Span::leadsSmoothlyTo(prev, next)` 为假时把该项**取负**——负号就是「这里是硬边，
法向不要跨过去平均」。闭合处（首尾 span）另有一次同样的判定。本期不实现，但这解释了
曲面法向该怎么分组，与 `d0088e93 fix(geom): smooth curved-surface normals` 是同一件事，
记在这里免得下次又从头找。

- **改法**：`src/fast_model/libgm_discretise.rs` 新增回转 / collar 口径的段数计算
  （配对取大 + 单调取大 + 全局重标定），输出 `Vec<i32>` 后逐 span 交给现有的
  `span_polyline_in_steps`。与挤出口径 `span_polyline_by_tol` **并列，禁止合并**
  （ADR-044 决策 3）。`tessellate_revolution` 改用前者。
- **门**：(a) 同一条轮廓在两个入口下段数不同的对照单测，钉住「两套口径确实不同」；
  (b) 一对反向重合边拿到相同段数（`pairedSpan` 生效）；
  (c) 人为把容差调到让 `total > 1000`，验证重标定后 `total` 落回 1000 附近而不是被截断；
  (d) 带孔回转截面内外壁段数按规则一致。
- **依赖**：F1（先把不该存在的分支拿掉再改口径）。

### G2 曲面原语段数由真实半径算出 — **2026-08-23 落地（T038 / T039），余椭圆碟经向**

按 §7.9.1 调用点表逐条替换写死值。半径已在参数里的先做（不动身份）：

| 原语 | 喂进去的半径 | 附加规则 | 状态 |
|---|---|---|---|
| SSCL 切角柱 | `radius`（`getNCoords` `0x1009E3A0`） | — | ✅ |
| `PrimLSnout` | `fmax(rBtm, rTop)`（`getNCoords` `0x1009F600`） | 不是底也不是顶 | ✅ |
| `PrimRTorus` | `partRev(rOut, tol, s, e)` | — | ✅ |
| `PrimCTorus` | 扫掠 `partRev(rOut, …)`，管截面 `circle((rOut−rIns)/2, …)` | 非整圈段数 **+1**（在环面生成器内部做，别加第二次） | ✅ |
| `PrimDish`（球碟） | `h ≥ a ? R : a`，`R = (a²/h + h)/2` | 经向 `ceil(θ/(2π/n))`，`θ = acos(1 − h/R)` | ✅ |
| `PrimDish`（椭圆碟） | 绕轴 `circle(a, tol)`——喂**底半径**，不是 `R_c` | 经向见下 | 绕轴 ✅ / 经向 ⛔ |

椭圆碟这一行 2026-08-24 反完，**结论是它不属于 G2**：libgm 的 `GM_EDish`
（`0x10054AB0`）是托里球形封头——球冠加一圈相切的环面拐角，而本仓
`gen_elliptical_dish` 当时画的是半个旋转椭球。**换段数换不出另一族曲面**，整个生成器
重做，同日随 T038a 落地（`libgm_discretise::elliptical_dish_facets` +
`mesh_primitives::TorisphericalArc`）。规则备查：

- `r_k = h / (1 + (a − h)/√(a² + h²))`；Core3D 现算后传入，`RADI` 只当开关、数值被丢掉。
- `R_c = radiusOfHub() = (a² + h² − 2a·r_k) / (2(h − r_k))`。
- **`θ = acos((R_c − h)/(R_c − r_k)) = atan2(h, a)`**。~~`acos((h − r_k)/(R_c − r_k))`~~
  是上一版记错的——那是 Hex-Rays 吞掉 acos 实参后的伪码，反汇编（`0x10054CCB`）是
  `acos(1 − q)`。
- `n_hub = partRev(R_c, tol, 0°, θ°)`、`n_knuckle = partRev(r_k, tol, θ°, 90°)`；
  封顶判据 `2(n_hub + n_knuckle) > 1000`，触发后 `4·n > 1000` 的那一段各自夹到 250。
- `isSpherical()`（`|a − h| ≤ 1e-6`）时 `θ` 直接取 45°，且 `R_c` 保持等于 `r_k`。

- **文件**：`src/fast_model/manifold_tessellate.rs`、`src/fast_model/mesh_primitives.rs`
  （`DEFAULT_CIRCULAR_SEGMENTS` 与散落的 `32` / `16` 已从生产路径清掉；剩下的两处
  写死收进了具名欠账 `unit_mesh_identity`，等 G3 换键时整组删）
- **门**：每个原语一张「半径 → 段数」对照表单测，数值手算自 §7.9.1；
  `mesh_primitives` 里不再有默认段数常量的源码断言。两条都已绿。
- **依赖**：无（这几类本来就烤真实尺寸，不碰身份）。可与 G1 并行。

### G3 改单位网格身份键 —— **2026-08-24：范围不止柱与球**

> 原标题写的是「柱与球」，那是按 2026-08-23 那次盘点里**有实例**的两类定的。
> 补测发现碟 / 圆环面 / 矩形环面 / 不偏心 Snout 同样是单位几何，同样在把单位半径
> 喂给权威段数规则。下面这一节的改法对它们逐字适用，但**爆炸半径与排期都要按五类
> 重估**——37 份 `.mesh` 是只算单位柱得到的数。见 T053 与
> `docs/evidence/2026-08-24-unit-normalised-curved-primitives.md`。

- **问题**：`LCylinder::hash_unit_mesh_params()` 返回常量 `CYLINDER_GEO_HASH`，
  `gen_unit_shape()` 返回 `Self::default()`；一份网格没法同时是 16 段和 484 段。
- **改法**（ADR-044 决策 2 / 5）：`hash_unit_mesh_params` 混入该实例算出的段数；
  `src/fast_model/pdms_inst.rs` 的 `canonical_unit_param_json` 里那条 `CYLINDER_GEO_HASH`
  特判跟着按新键走。
- **门**：不同半径的两根柱子拿到不同 `geo_hash` 且各自网格段数正确；
  同段数的两根仍共享一行（复用没丢）；`canonical_unit_param_json` 不得产生双键 `param`
  对象（沿用 2026-08-13 那条回归测试）。
- **依赖**：G2（先确定段数怎么算，再把它写进键）。**这一步改身份，必须与 WP-I 的
  盘点结论一起决策**——爆炸半径要先量出来。

### G4 容差单一来源 — **2026-08-23 完成（T042）**

- **改法**：`FACET_TOL_MM` 是唯一容差来源；禁止回到 `PdmsGeoParam::tol()` 的相对容差。
  要做成 `DbOption` 可配时仍只有一个来源。
- **门**：源码断言生产路径不调用 `PdmsGeoParam::tol()` 喂段数。
- **落地时发现这不是一条纯断言任务**：`sweep_mesh::sweep_solid_mesh` 一直用
  `sweep.tol()`（= 0.01 × 轮廓外接球半径）喂 `profile_loops` → `arc_segments`。
  比例容差让 `tol/R` 恒定，段数与构件尺寸无关——**同一个半径的弧，在墙上和在与它
  相交的原语上会分成不同段数**，而 §6.11 的 `cancelFacets` 只消全等重叠。这是 G2 那条
  论证在扫掠体上的同一个病，只是此前没人量过墙这一侧。已改成全局绝对量。
- 常量从 `manifold_tessellate` 迁到 `libgm_discretise`：段数规则与它喂的容差住在一起，
  「全库唯一一份」才不只是句注释。模块文档里「我们这边仍是每个原语按自身尺度给
  `tol()`，口径尚未对齐」同步作废。
- **代价与缺口**：墙的弧段段数会变，而能量它的是 WP-J 的 RVM 门，**要等 H1 解绑才跑
  得起来**。目前只有纯函数单测与体积门覆盖。这一条不要当成已验收。

---

## WP-H  验收能力解绑（ADR-030 决策 10）

### H1 `mesh_compare` 的 gen 侧脱离 `occ` — **代码 2026-08-23 落地（T043），门只验了一半**

- ~~**现状**：`mod gen_side` 整个挂在 `#[cfg(feature = "occ")]` 下。~~ 已改成
  `#[cfg(feature = "manifold")]`；gen 侧形状由生产同款 `tessellate_libgm_param` 裁决，
  OCC 降为 `#[cfg(feature = "occ")]` 包住的可选参照分支。
- **门的前一半已验**：CI 口径（`ws,gen_model,manifold,project_hd`，不带 `occ`）
  `--lib` 编译通过。
- **门的后一半没验**：「跑出对拍结果」还没发生过。它就是 WP-J 那四道门本身
  （T046–T049），不是 H1 能自己收掉的东西。**别把 H1 的勾读成「验收能力已经有了」**
  ——编得过只是必要条件。

### H2 历史断言与浸水死代码 — **2026-08-24 完成（T044）**

- ~~`src/fast_model/loop_model.rs` 那条 `occ` 断言改成对 manifold 结果断言。~~ 已改：
  原 `occ` 块在 CI 口径下根本不编，是一条从来没跑过的断言。
- ~~`src/plug_in/water_calculation.rs` 是死代码，删之前确认 STP 路径确实是空的。~~
  已确认并删除：`opencascade_rs` 在任何 `Cargo.toml` 里都没定义过，`export_stp` 的
  唯一调用点连自己的 `mod` 声明都是注释。整个模块随后由另一会话一并删除，
  重启路径以 `changelog.md` 那条为准。

### H3 四道 RVM 门补跑

扫掠体 2026-08-20 已上生产但一条门没跑。直墙 / 弧墙 / 斜切墙 / 360° SANN 各一条，
阈值不放宽（FR-010），证据进 `docs/evidence/`，live 台账同步。

---

## WP-I  活库盘点（FR-008）—— **2026-08-23 已完成**

同日跑了**两个库**，互为交叉验证。定性结论完全一致；定量数字按库规模不同，排期一律
取大的那个。

| | 库 A（大） | 库 B（小） |
|---|---|---|
| 数据源 | `.surreal/ams-7997-e3d-test-20260805` 的副本 | `@8009` |
| `inst_geo` 行数 | 8,094 | 3,637（带 `param` 的 2,108） |
| 展平实例条目 | 214,847 | — |
| 证据 | `docs/evidence/2026-08-23-occ-retire-census.md`（含全部查询与原始导出） | 本节表格 |

正式库 `.surreal/ams-8009` 已被 3.x 写坏且决定不修（AGENTS.md），两次都没碰它。
库 A 走 `bin/surreal.exe` 2.1.4 打开只读副本，盘点后副本与进程已清理。

### 变体分布

「行数」是 `inst_geo` 去重后的单位网格身份数；「实例数」库 A 取
`inst_relate.insts_flat[]` 条目，库 B 取 `geo_relate` 边数。

| 变体 | A 行数 | A 实例 | B 行数 | B 实例 |
|---|---:|---:|---:|---:|
| `PrimExtrusion` | 3,896 | — | 2,007 | 6,687 |
| `<absent>`（布尔产物 / mesh-only，无 `param`） | 2,942 | — | 1,529 | — |
| `PrimLoft` | 567 | — | 1 | 4 |
| `PrimRTorus` | 167 | — | 16 | 67 |
| `PrimPyramid` | 158 | — | 0 | 0 |
| `PrimLSnout` | 112 | — | 3 | 6 |
| `PrimCTorus` | 95 | — | 4 | 5 |
| `PrimLPyramid` | 77 | — | 33 | 69 |
| `PrimRevolution` | 61 | — | 42 | 122 |
| `PrimDish` | 17 | — | 0 | 0 |
| `PrimBox`（单位箱） | **1** | 16,725 | **1** | 1,024 |
| `PrimLCylinder`（单位柱） | **1** | 21,354 | **1** | 99 |
| `PrimSphere` / `PrimSCylinder` / `PrimPolyhedron` | 0 | 0 | 0 | 0 |
| `Unknown` / `CompoundShape` | 0 | 0 | 0 | 0 |

箱与柱各只有一行、却带着上万个实例——这就是单位网格身份（ADR-026）本身，也正是
G3 要改的那把键。

库 B 的 `PrimLoft` 只有 1 行 4 实例，曾让人怀疑扫掠体是否根本不落 `param`；库 A 的
567 行否掉了这个怀疑——**扫掠体确实落 `param`**，库 B 只是结构件少。墙类的正确性仍由
WP-J 的 RVM 门覆盖，不靠本表。

### 三个专项：全为 0，三条收口都是纯防御

| 专项 | 库 A | 库 B | 结论 |
|---|---|---|---|
| 样条挤出（`cur_type` 为 `Spline`） | 0 / 3,896（全是 `Fill`） | 0 / 2,007 | T036 纯防御 |
| 出平面回转轴 | 0 / 61（`rot_dir` 全等于 `[1,0,0]`） | 0 / 42 | T033 纯防御 |
| `Unknown` / `CompoundShape` | 0 | 0 | T037 纯防御 |

库 B 的出平面判定用的是 `abs(rot_dir[2]) > 1e-4 OR abs(rot_pt[2]) > 1e-3`，与
`tessellate_revolution` 现有判定同构。**三条都不会打掉任何现存构件**，可以先做、
不必等 RVM 门。

### G3 爆炸半径：可忽略，本期做

| | 库 A | 库 B |
|---|---|---|
| 单位柱实例 | 21,354 | 99 |
| 不同半径 | 295（3 – 2,658 mm） | 14（0.5 – 324.5 mm） |
| 折成的段数等价类 | **37**（8 … 164） | **7**（`8×38, 16×34, 20×22, 24×1, 28×2, 56×1, 60×1`） |
| 改键后 `.mesh` 份数 | 1 → 37 | 1 → 7 |
| 单位球 | 0 实例，不受影响 | 0 实例，不受影响 |

理论上限是 249（4 的倍数、`[8, 1000]`）；实测最坏 37，而 `inst_geo` 里已经躺着 3,896
行挤出。**裁决：T041（G3）本期做**，排期按 37 算。整库重建走 ADR-021 的既有回退路径。

### 写死 32 段到底错得有多厉害

两个库从两个角度量，指向同一结论：

- **库 A（按实例加权）**：段数恰好该是 32 的只有 429 个，**2.0%**；7.1% 过粗，
  **90.8% 过细**。九成是过细——DN15 一类小口径 E3D 只给 8–12 段，我们给 32，
  白多算三角的同时还跟 E3D 对不上。
- **库 B（按弦高）**：两根大半径柱（r = 295 / 324.5 mm）在 32 段下弦高是
  1.42 / 1.56 mm，**超出 `FACET_TOL_MM = 0.5` 约 3 倍**；另有 38 个半径 ≤ 6.6 mm 的
  实例被过采约 4 倍。

所以 G2/G3 既不是画质优化，也不只是「跟 E3D 对齐」的洁癖：**它同时是容差正确性**
（大柱超容差 3 倍）和**布尔正确性**（`cancelFacets` 只消全等重叠，§6.11）。

### 偏心 Snout 补测（2026-08-24，T052）

WP-K 的两条缺陷只影响 `poff != 0` 的 Snout，上一轮盘点没拆这一列，补测两个库：

| | 库 A | 库 B |
|---|---:|---:|
| `PrimLSnout` 行数 | 112 | 3 |
| 其中偏心（`\|poff\| > 1e-6`） | **0** | **1** |
| 偏心实例数 | 0 | **2** |

库 A 的 112 行 `poff` 全为 `0.0`（都是靠 `ptdm/pbdm` 比值区分的单位锥台）；库 B 那一件
`poff = 12.06`、`pbdm/ptdm` 66.33/84.42、高 115.2，是真实尺寸不入复用。
**裁决：T050 本期做。** 数量微不足道，但错的是位置——那一件被整体挪了 6.03 mm，
是 `FACET_TOL_MM = 0.5` 的十二倍，FR-010 不许靠放宽阈值过门。它同时是个现成的
验收样本，塞进 T049 的曲面原语抽检即可。

顺带：两库全部 115 行的 `paax_dir` 恒 `[0,0,1]`、`pbax_dir` 恒 `[1,0,0]`，所以 `poff`
就是 XOFF。但「有没有 YOFF ≠ 0 的构件」**在 `inst_geo` 上问不出来**——`LSnout` 只有
一个 `poff` 字段，源数据里的 YOFF 落库时就没了，查出 0 不构成证据。T051 的前置因此
换成 T052a（回 dabacon 侧数）。证据：`docs/evidence/2026-08-24-eccentric-snout-census.md`。

> **这一轮翻了上一轮的一个隐含预期。** T045 的三个专项在两个库都是 0，于是「两库
> 定性一致」成了默认假设；这次两库结论相反（A 全 0、B 有 1）。再有「预期为 0」的
> 专项，**一个库查出 0 不能当证明**。

### 遗留缺口

两个库都**没有球 / 切角柱（SSCL） / 多面体**，这三类的段数改动**无法在现场验收**。
另找含这些原语的库，或只以纯函数单测收口并在此明确注明「未经现场验证」——
不得默认它们跟圆柱一样安全。已开 T045a 钉住。

椭圆碟同理：库 A 17 行 / 库 B 0 行的 `PrimDish` 没拆球碟与椭圆碟，T038a 换完形状族
能不能现场验收存疑，按同一口径处理。

---

## libgm 符号对照（保留自上一版，供实现时查）

Core3D IAT 签名（stdcall 修饰名已核对）。单位几何列「是」的，生成固定信封，真实尺寸
走实例变换。

| 符号 | 签名 | 本仓对应 |
|---|---|---|
| `gm_CreateBox` | `(double,double,double)` | `PrimBox` → 单位箱 |
| `gm_CreateCylinder` | `(double height, double radius)` | `PrimLCylinder` / 无切角 `PrimSCylinder` |
| `gm_CreateSphere` | `(double radius)` | `PrimSphere` |
| `gm_CreateSnout` | `(double×5)` | `PrimLSnout` |
| `gm_CreateCircularTorus` | `(double×4)` | `PrimCTorus` |
| `gm_CreateRectangularTorus` | `(double×5)` | `PrimRTorus` |
| `gm_CreateSphericalDish` / `gm_CreateEllipticalDish` | `(double×2)` / `(double×3)` | `PrimDish` |
| `gm_CreatePyramid` | `(double×7)` | `PrimPyramid` / `PrimLPyramid` |
| `gm_CreateSlopeEndedCylinder` | `(double×6)` | 切角 `PrimSCylinder` |
| `gm_CreateExtrusion` | `(unsigned profile, double height)` | `PrimExtrusion`；扫掠 C1 |
| `gm_CreateRevolution` | `(double,double,D2_Point const&,double,unsigned)` | `PrimRevolution`；扫掠 C2 |
| `gm_CreateRuledSolid` | `(double len, unsigned profA, unsigned profB)` | 扫掠 C3 |
| `gm_CreatePolyhedron` + `gm_Add*` | — | `PrimPolyhedron` |
| `gm_CreateCombination` | `(GM_Operation)` | manifold `batch_union` / `batch_difference` |
| `gm_CreateNull` / `Mark` / `Straight` / `Arc` / `Bezier` | — | **不进生产**，辅助几何 |

**明确不移植**：`gm_CreateSection` / `CutSurface` / `ClippedTree` / `ExpandedTree` /
`CompressTree` / `SolidTree` / `NormalisedItem`（树优化，不是新图元）；
`gm_QueryClash` / `Close` / `XRay` / `Mass` / `gm_Picture*`（碰撞与出图）；
`gm_CreateBody` / `AM_CoEdge`（B-rep 内核）；`gm_CreateIterator` 族（Rust 直接持有
`Vec<Manifold>`）。离散一律 `Manifold::to_mesh_f64`，不调 `gm_QueryFacetData`。

## 推荐实施顺序

```
✅ WP-I  活库盘点            ✅ WP-F 假回退收口
✅ WP-G1 回转轮廓口径        ✅ WP-G2 曲面段数（不碰身份）
✅ WP-G4 容差单一来源        ✅ WP-H1 mesh_compare 解绑（T043，编得过）
                             ✅ WP-H2 历史断言 + 浸水死代码（T044）
──────────────── 以下未完 ────────────────

三条实现线，互不依赖（纯函数那半进得了 CI，2026-08-24 已经跑掉两条）：

  ├─ WP-G3 单位网格身份键（T041；改身份 = 整库重建，ADR-021 回退路径）
  │        └─ T053 先把范围数清楚：碟 / 两种环面 / 同心 Snout 也是单位几何
  │                （只查库，无代码依赖，先于 T041 排期）
  ├─ WP-K  形状摆位对齐（T052 已量：偏心 1 件 2 实例，错位 6.03 mm）
  │        ├─ T050 偏心 Snout 半偏移 ✅ 代码两侧都改了，缺 RVM
  │        └─ T051 YOFF 接通 ⏸ 不排期，前置 T052a（得回 dabacon 侧数 YOFF）
  └─ T038a 椭圆碟换成托里球形封头 ✅ 已落地，缺现场样本（活库 0 件椭圆碟）

           └──────────── 三条都收敛到同一个闸 ────────────┐
                                                          ▼
                        ★ WP-J 把 RVM 门真的跑起来（T046–T049）
                          四道墙体门 + 曲面原语抽检；阈值不放宽（FR-010）
                                       │
                                       └─ Phase 4：default/release 去掉 occ
```

**当前的卡点不是「编不过」而是「没跑过」。** 上一版把 WP-H1 排在最前、写成硬阻塞，
那在 T043 落地之前是对的；现在解绑已经完成，尺子拆下来了，但**一次都还没量过东西**。
四道 RVM 门是 2026-08-20 扫掠体上生产时就欠着的，到现在一条没跑，而后来又压上了
T041 / T050 / T038a 三件「改完必须对拍才敢认」的改动——它们全排在同一个闸后面。

树上那三条线是**验收**依赖而不是**实现**依赖：体积对解析值、切向连续、段数对照表
这些纯函数门不连库，可以先写先绿。区别在于写完**不能勾**——曲面族换掉、身份键换掉、
摆位换掉，都是「看起来对」和「跟 E3D 一样」之间差着一次对拍的事。顺序上可以抢跑，
勾选上不行。

> 上图只排工作包之间的先后。**落到人和分支上的排期见
> `docs/plans/2026-08-24-occ-retire-endgame-plan.md`**，那份把 vendor 侧
> （`../vendor/old-aios-core/src/prim_geo/snout.rs` 上同时压着 T002 的规范化修复与
> WP-K 的两条）串成一条链，要求一次推上游、别分三次改同一个文件。两份出现分歧时
> 以本文件的**依赖关系**与那份的**执行顺序**各自为准；若连依赖关系都对不上，
> 说明有一份没跟上 `tasks.md`，先对台账。

## 验证

- 每包：纯函数单测（不连库、进得了 CI）。空输入必须红。
- 段数：每个曲面原语一张「半径 → 段数」对照表，数值手算自 §7.9.1，不许反向从实现取值。
- 扫掠体：直墙 / 斜切墙 / 弧墙 / 360° SANN 走既有 RVM 门，不放宽阈值。
- 布尔：`pe:17496_116569` p95 ≤ 180。
- `cargo fmt`、`cargo check`；禁止 `cargo clean`。
- live 更新 `docs/2026-08-12_live-test-ledger.md`。
- 本地 vendor patch 不得推 main；aios-core `gen_model` 与 `manifold-sys` 解耦要先合上游。

## 明确不做

- 通用 3D 脊线扫掠（超出扫掠三支）。
- OCC 布尔回生产。
- 把 `gen_manifold_mesh` 只放在 gen-model 长期不回 aios-core。
- 复刻 libgm 的 handle / iterator / picture / clash。
- 为了过门放宽 RVM 阈值，或把段数「取大一点凑合」——段数要的是相等，不是够多。
