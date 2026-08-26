# ADR-030：分阶段退役 OCC 三角化，default/release 暂不删 `occ`

状态：Accepted（2026-08-15）

关联：ADR-002（几何权威在 core3d / 已解析参数）；ADR-026（扫掠体步骤与单位网格身份）；
ADR-029（布尔改走本地 manifold-csg，OCC 只留三角化）。术语见 `CONTEXT.md`
「扫掠体 / 单位几何 / 实例变换 / 单位网格身份 / 规范挤出 / 斜切平面」。

规格：`specs/009-retire-occ/spec.md`。

## 背景

生成管线是两套内核：三角化走 OCC `gen_occ_shape` → `gen_occ_mesh`；布尔已按
ADR-029 走本地 `manifold-csg`。OCC 布尔（`apply_insts_boolean_occ`）生产路径已注释；
2026-08-15 对 `pe:17496_116569` 的 `BRepAlgoAPI_Cut` 写出 60 字节空网格，不得回生产。

`Cargo.toml` 的 `occ` 仍在 default。关掉它时 `gen_inst_meshes` 直接 `Ok(())` 并打一句
warning——这是静默跳过，不是安全退役。manifold-csg 能做直线挤出、绕轴旋转和网格 CSG，
**没有沿任意 3D 脊线的扫掠，也没有两条三维线之间的 ruled loft**。WALL / STWALL /
GENSEC 的几何是扫掠体（PrimLoft），仍绑在 OCC 上。

一批目录图元（圆环面、PrimLSnout、碟、锥）在 **manifold-csg 现成 API** 里没有同名函数，
但它们在 Core3D 里是 **libgm 一等原语**（见下方 IDA 修订），应用 `gm_CreateSnout` 等
语义在 manifold 上实现，而不是当成永久 BLOCKED。`water_calculation` 仍调
`insts_data.gen_occ_shape`，可能依赖形状拓扑，是第二个抑留点。
（**2026-08-24 作废**：浸水插件已整个删除，不再是抑留点，见下方决策 11。）

## IDA 修订（2026-08-15，Core3D.dll.i64 / `idalib-32268`）

E3D **不用 OCC**。三角化与实体的权威链是：

```
core.dll          分类 / 包围盒 / 视图段（不建实体）
    ↓ noun 谓词
Core3D.dll        DB_Gensec / CSG_TreeBuilderCat 调度
    ↓ gm_Create*
libgm             实体 + CSG 树 + 离散（gm_QueryFacetData / PictureDraw）
```

已核对的入口（Core3D 3.1）：

| 符号 | 地址 | 作用 |
|---|---|---|
| `DB_Gensec::do_solid_segments` | `0x10732FF0` | 取目录截面 → `setMitrePlanes` → `setSpineSegmentTransforms` → `setImpliedBangs` → 逐段建体 |
| 逐段建体 | `0x107318E0` | `gm_CreateRevolution(..., 180.0, profile)` / `gm_CreateExtrusion(profile, len)` / `gm_CreateRuledSolid(len, profA, profB)`；斜切是额外挤出再 `gm_AddMember` |
| `CSG_TreeBuilderCat::getCSGTree` | `0x1072F5D0` | `gm_CreateCombination` + 子图元 `gm_AddMember`；负体走 `addNegatives` |
| libgm 原语 IAT | `0x10AEC438` 起 | Box / Cylinder / Snout / CircularTorus / RectangularTorus / Sphere / SphericalDish / EllipticalDish / Pyramid / SlopeEndedCylinder / Extrusion / Revolution / RuledSolid / Polyhedron / Combination |

OCC 是我们自己把 `gm_Create*` 翻成 BRep 的翻译层。退役目标是 **对齐 libgm 原语表**，
不是对齐 OCC 的 loft/revolve API。

## IDA 修订二（2026-08-23，libgm.dll 3.1 / `idalib-18608`）

上一版翻的是 Core3D 的**调用点**。这一版翻的是 libgm 自己的**实现**，结论推翻了 T007a /
T007b 留下的那三个「有意的 OCC 回退口子」：三个都挡的是 libgm 表达不出、我们的解析器也
造不出来的输入。它们不是回退，是伪装成回退的死分支。

### 一、回转轴在 libgm 里是二维的，没有「出平面轴」这回事

| 符号 | 地址 | 事实 |
|---|---|---|
| `gm_CreateRevolution` | `0x1003A580` | `(double, double, D2_Point const&, double, unsigned)` |
| `GM_Revolution::GM_Revolution` | `0x10033830` | `(double startAngle, double finishAngle, D2_Point const& origin, double axisAngle, GM_Profile*, double tol)` |
| `translatePolygonIntoStandardPosition` | `0x10097810` | `moveBy(−origin.x, −origin.y)`；`axisAngle != 90` 时 `rotateBy(90 − axisAngle)` |
| `movePointsOntoYAxis` | `0x100978A0` | 顶点 x 的绝对值小于 `GM_User::normtol_` 时把 x 置 0 |

轴是**轮廓平面内的一个角度**，不是三维向量；标准位是「原点搬到零点、轴摆到 +Y」。
我们这边 `Revolution` 的唯一构造点是 aios-core `prim_geo/category.rs`，走
`..Default::default()`，`rot_dir` 恒为 `Vec3::X`。所以出平面轴既非 E3D 能画出的形状，
也非本仓解析器能产出的状态——它属于宪法「响亮失败」条，不属于「显式回退」。

`movePointsOntoYAxis` 那一下**吸附**我们还没抄：轮廓上贴近轴的顶点在 E3D 侧被精确压到
轴上，不压就会在轴心留下一圈针状面。这是本次新发现的欠项，不是既有实现的等价改写。

### 二、`CurveType::Spline` 既不是样条，也没有任何代码构造得出来

`Extrusion::cur_type` 的 `Spline(thick)` 变体在整个工作区（gen-model + 三个 vendor +
全部 worktree）**没有一处构造点**，只出现在单测里。而它对应的 OCC 实现
`wire::gen_occ_spline_wire` 读下来根本不是 NURBS：要求恰好 3 个 SPINE 点
（起点 / 过渡点 / 终点），解出三点圆，按 `thick` 一半向内外偏移，拼成
「外弧 + 直段 + 内弧 + 直段」的环形扇区——就是一段**弧形墙的截面**。

libgm 侧同样没有样条轮廓：`D2_Profile` 的 span 只带 bulge；`GM_Bezier`
（`gm_CreateBezier` `0x10038880`，构造 `(D3_Point, D3_Point, D3_Point, double, double)`）
是三点加权的**曲线图元**，靠 `gm_AddCurve`（`0x1003A7D0`）挂进树里，走
`calcFacetsWithoutSurfaces` 出折线，不参与实体轮廓。

### 三、`Unknown` / `CompoundShape` 从来没走过 OCC

两者的 `check_valid()` 都返回 `false`，而 `PdmsGeoParam::gen_occ_shape()` 第一句就是
`if !check_valid() { return Err("Invalid shape") }`。回 `None` 只是把同一个失败挪到
另一个函数里报，删 `occ` 对它们零影响。

### 四、真正的缺口：回转轮廓的离散规则跟挤出**不是同一条**

`GM_Revolution::calcFacetsWithoutSurfaces`（`0x10097920`）的被调列表里有
`GM_Profile::polygonForFacet`（`0x1008ED80`），后者走 `GM_Profile::setNSteps(double)`
（`0x1008F2E0`）。反编译该函数：逐 span 取自身半径 `D2_Span::getRadius`，若该 span 有
配对 span（`GM_Profile::pairedSpan` `0x1008F7F0`）则一并取其半径，
`n = d2_numberOfSegmentsForCircle(fmax(自身半径, 配对半径), tol)`，写回时与已存步数
**取大**（只增不减）。

`polygonForFacet` 的调用方只有 `GM_Collar` 与 `GM_Revolution`（含各自的 `validate`）——
**`GM_Extrusion::calcFacets`（`0x10056F10`）不在其中**，它走的是每 span 自算的
`D2_Span::getApproxPolyLine`。也就是说 libgm 有两套轮廓离散口径，而本仓
`tessellate_revolution` 目前把挤出那一套用在了回转上。REVO / NREV 是 PANE 负实体的主力，
且已在生产路径上；段数差一段，`cancelFacets` 的共面抵消就整个放弃（§6.11），
结果是布尔后留一层内壁。

### 五、单位网格身份与 libgm 的半径相关段数直接冲突

libgm §7.9.1 的调用点表说明每个曲面原语喂进 `d2_numberOfSegmentsForCircle` 的是**真实
半径**。本仓 `tessellate_libgm_param` 里柱 / 球 / PrimLSnout / 碟 / 圆环面 / 矩形环面 /
斜端柱的段数全是写死的 32（球是 16×36）——`libgm_discretise` 的单测已经写明 32 只在
「R=100 配 0.5mm 容差」这一个尺寸上对。

其中柱与球走**单位几何**：`LCylinder::hash_unit_mesh_params()` 返回常量
`CYLINDER_GEO_HASH`，`gen_unit_shape()` 返回 `Self::default()`，全库所有圆柱共享一行
`inst_geo` 和一个 `.mesh`。这份身份无法承载随半径变化的段数——要按 libgm 出段数，就得
改身份键。该决策超出本 ADR 范围，另立 ADR-044。

## 决策

1. **不在本期从 default / release 拿掉 `occ`。** 删 `occ` 的前提是：生产路径上每个会
   走到的 `PdmsGeoParam` 都能直接稳定生成 `PlantMesh`，并通过既有 RVM 对拍门。不是
   「所有 OCC API 都换完」。
2. **三角化后端权威放在 aios-core。** 新增 `gen_manifold_mesh(&PdmsGeoParam) -> PlantMesh`。
   gen-model 只负责调度、布尔、AABB、缓存、落盘。`occ_generate.rs` 改成后端 trait
   调度，不得整文件删除。
3. **扫掠体对齐 libgm 三支，不做通用脊线扫掠。** `do_solid_segments` 逐段调用
   `gm_CreateExtrusion` / `gm_CreateRevolution`(实测 180° 半圆再组合) /
   `gm_CreateRuledSolid(len, profileA, profileB)`。斜切平面只改端面，另用挤出延伸
   （`Start-mitre extension` / `End-mitre extension`）再挂到 CSG 树上。不得发明
   第三条「任意 3D 脊线」内核。Core3D 的斜切平面 / BANG / PLAX / 单位网格身份
   （ADR-026）不得改语义。
4. **目录图元按 libgm 一等原语实现，不因 manifold-csg 没有同名 API 标 BLOCKED。**
   Snout / torus / dish / pyramid / slope-cylinder 用挤出、旋转、凸包或解析网格
   复刻 `gm_Create*` 参数表；未实现完之前生产仍回退 OCC，但盘点的目的是排期，不是
   永久放弃。
5. **`not(occ)` 且没有替代生成器时必须失败。** `gen_inst_meshes` 不得再 `Ok(())` 静默
   跳过。空网格不得覆盖已有 `booled_id`。空轮廓挤出（如 NXTR `17496_116867`）hard fail。
6. **浸水插件不挡主生成管线删 OCC。** 先调查它对 BREP 面/点分类的依赖；能改网格 CSG
   就改，不能则独立 feature，不进最终 release default。
7. **回滚只加回 `occ=true`，不恢复 OCC 布尔。** 禁止 `cargo clean`。

### 决策修订（2026-08-23，依据「IDA 修订二」）

上面七条的意图不变，下面三条是按新证据收紧的口径：

8. **`tessellate_libgm_param` 不得再有「回退 OCC」的 `Ok(None)`。** 三个现存回退口子
   按证据重新归类：出平面回转轴 → `bail!`（libgm 表达不出，属决策 5）；
   `CurveType::Spline` → 按弧形墙截面在 manifold 上实现（它不是样条）；
   `Unknown` / `CompoundShape` → 直接标 `bad`，不假装有后端可退。
9. **离散段数是布尔的前置条件，不是画质旋钮，因此进决策 1 的出口判据。** 「能稳定生成
   `PlantMesh`」不足以放行；曲面原语的段数必须按 libgm §7.9.1 的调用点表由**真实半径**
   算出，回转轮廓必须走 `setNSteps` 那一套（配对 span 取大 + 单调取大），
   不得沿用挤出的每 span 自算。
10. **删 `occ` 之前，量尺子的工具必须先脱离被量的对象。** `rvm_baseline/mesh_compare.rs`
    的 `gen_side` 整个挂在 `#[cfg(feature = "occ")]` 下；先把它改成 manifold 为准、
    OCC 仅作可选参照，否则删 `occ` 会连同验收能力一起删掉。

### 决策修订（2026-08-24）

11. **决策 6 以「删除」收口，不再调查、不再独立 feature。** 浸水插件
    （`src/plug_in/water_calculation.rs`）业务上已不需要，整个模块删除。
    删除时它对 OCC 的依赖其实早已不存在：唯一那处 BRep STP 导出挂在 feature
    `opencascade_rs` 下，而这个 feature 在任何 `Cargo.toml` 里都没定义过，历来所有
    构建编的都是那个写死字符串的占位实现，死分支已于 2026-08-23 按决策 6 删掉。
    剩下的四个 ArangoDB 查询函数与 `save_stp_data_to_arangodb` 在仓内**零调用点**，
    连 `AQL_WATER_CALCULATION_COLLECTION` 也只有它自己在用。
    背景一节里「第二个抑留点」的说法自此作废，OCC 抑留点只剩扫掠体（PrimLoft）一处。
    浸水若重新立项，从 spec 起步，不要从 git 里捞这份（它的 STP 导出从未发布过）。

12. **决策 7 作废：回滚口径改为 git revert。** 「回滚只加回 `occ=true`」自 T037 起
    就不再成立——形状回退拆除后，带 `occ` 的构建形状也只走 manifold，把 feature
    加回来什么都不会变。X1a（gen-model 摘 `occ` feature 与 `dep:opencascade`，
    `56ce58c6` + `8cf820ab`）与 X1b（aios-core 摘 `occ` feature、opencascade 依赖
    与 22 个文件的翻译层，aios-core `b546648c`）落地后，这个开关本身已不存在。
    真回滚 = revert 对应提交并沿 rev bump 链回退。「后果」一节第二条
    （aios-core / gen-model 仍保留 occ）自此成为历史陈述；RVM 门（WP-J）仍欠的
    验收账不因摘除而免——见 specs/009 tasks T046–T049。

分期（出口见 spec）：Phase 0 失败闭合；Phase 1 布尔（ADR-029，进行中）；Phase 2 可盖
原语；Phase 3 扫掠体网格器；Phase 4 才从 default/release 删除 `occ`。

## 后果

- 布尔失败不再被理解成「OCC 没开」；三角化缺口也不再被理解成「manifold 切洞坏了」。
- aios-core 在 Phase 4 之前仍编译 `opencascade`；gen-model default 同样保留 `occ`。
- 斜切墙的延伸挤出与 ruled 两截面是 libgm 语义，不是 OCC loft。
- 圆环面 / PrimLSnout / 碟 / 锥按 `gm_Create*` 排期实现，未完成前仍回退 OCC。

## 否决方案

- 一次从 default/release 删除 `occ`：扫掠体与 BLOCKED 原语会停生成，或更糟地静默跳过。
- PrimLoft 永远独留 OCC：release 永远删不掉依赖，只是把问题藏进 feature 矩阵。
- 对齐 OCC BRep API（`Solid::loft` / `Face::revolve`）而不是 libgm `gm_Create*`：OCC 只是翻译层。
- 把 `gen_manifold_mesh` 只放在 gen-model：几何权威会再次离开 aios-core / Core3D 口径。
- 用 OCC 布尔补薄片或空结果：已被 116569 证伪。
- 放宽 RVM 对拍阈值来掩盖后端切换：把漂移藏进门控。
