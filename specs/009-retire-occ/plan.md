# Implementation Plan：按 libgm 原语表用 manifold-csg 替换 OCC

**规格**：`specs/009-retire-occ/spec.md`
**决策**：ADR-030（含 Core3D.dll IDA 修订）、ADR-026、ADR-029
**IDA**：`D:\AVEVA\Everything3D3.1\Core3D.dll.i64`（`idalib-32268`）
**内核权威**：libgm `gm_Create*`，不是 OCC BRep。全程只用本地 `manifold-csg`。

## Constitution Check

- **水位承诺**：本计划不改水位 / 队列 / 暂存窗口。
- **单一规则**：三角化只经 `tessellate_libgm_param`（及后续扫掠入口）；布尔只经
  manifold-csg。不得再长出第三条 OCC 布尔或第二套挤出。
- **响亮失败**：未实现的 `gm_Create*` 在关 `occ` 时失败；空轮廓 / 空网格 hard fail。
- **队列收口**：不新增队列 action。
- **标识真值**：不把 geo_hash 当 dbnum；单位网格身份仍按 ADR-026。
- **可执行守护**：每个工作包有纯函数单测；扫掠体与目录件有 RVM 门。

Complexity Tracking：Phase 4 之前 OCC 回退仍在 default——不是第二套生产内核，是未覆盖
类型的显式回退。aios-core `gen_model` 不再捆绑 `manifold-sys`（与 manifold-csg 链接
冲突）；该改动目前靠本地 vendor patch，推 main 前必须先合进 aios-core 再升 rev。

## 总原则

1. 每个 libgm 符号对应一个工作包：签名、manifold 映射、文件、测试、依赖。
2. **不移植** `AM_Body` / `gm_PictureDraw` 回调式离散。离散一律 `Manifold::to_mesh_f64`。
3. **不移植** clash / x-ray / mass / picture 隐藏线。那些不是生成管线。
4. CSG 树用 `Manifold` 值本身，不复刻 libgm 的无符号 int handle。
5. 单位箱/柱/球保持现有信封（尺寸在实例变换里），与 `BOX_SHAPE` / `CYLINDER_SHAPE` /
   `SPHERE_SHAPE` 一致。

```
PdmsGeoParam / SweepSolid / GMSET
        ↓  本计划的适配表
   manifold-csg Manifold
        ↓  to_mesh_f64
     PlantMesh → 落盘 / AABB / 布尔
```

---

## WP-A  基础设施（进行中）

| 项 | 内容 |
|---|---|
| 已有 | `manifold_csg.rs`（ingest / batch_difference / 索引网格）；`manifold_tessellate.rs` 箱/柱/挤出单测绿 |
| 还要 | `tessellate_libgm_param` 接到 `gen_inst_meshes`（仅已覆盖类型；其余 OCC 回退）；`not(occ)` 无后端则 `bail!` |
| 文件 | `src/fast_model/manifold_tessellate.rs`、`occ_generate.rs`、`manifold_csg.rs` |
| 门 | 现有 tessellate 五测；接生产后抽一箱一柱一挤出 live 网格非空 |
| 依赖 | 无。必须先于所有 WP-B/C |

---

## WP-B  目录一等原语（`gm_Create*` → Manifold）

Core3D IAT 签名（stdcall 修饰名已核对）。单位几何列「是」的，生成固定信封，真实尺寸走实例变换。

### B1 `gm_CreateBox(double,double,double) → id`  — 已有骨架

- **映射**：`PrimBox` → `Manifold::cube(1,1,1, center=true)`（对齐 `box_centered(1,1,1)`）
- **文件**：`manifold_tessellate.rs`
- **门**：`unit_box_is_non_empty`、`prim_box_param_uses_unit_mesh`（已绿）
- **并行**：可与 B2/B3 同时做

### B2 `gm_CreateCylinder(double height, double radius) → id`  — 已有骨架

- **映射**：`PrimLCylinder` / 无切角 `PrimSCylinder` → `Manifold::cylinder(1, 0.5, 0.5, segs, center=false)`
- **门**：`unit_cylinder_is_non_empty`（已绿）
- **缺口**：带 `btm_shear_angles` / `top_shear_angles` 的 SCylinder **不是** 本符号，转 B8

### B3 `gm_CreateSphere(double radius) → id`

- **映射**：`PrimSphere` → `Manifold::sphere`，单位球半径 0.5（对齐 `SPHERE_SHAPE`）
- **文件**：`manifold_tessellate.rs`
- **门**：单位球非空；半径退化 hard fail
- **并行**：可与 B1/B2 同时

### B4 `gm_CreateSnout(double,double,double,double,double) → id`

- **映射**：`PrimLSnout`。五个 double 需在 Core3D 调用点钉死（通常是底半径/顶半径/高/偏心 x/y）。manifold：`cylinder(h, r0, r1, segs, false)` + 顶面平移（偏心）或 `extrude_with_options(scale)`。
- **IDA 下一刀**：对 `0x1073c12c` 的 `xrefs` 反编译一个目录 SNOUT 工厂，记下参数顺序。
- **门**：正圆锥、偏心鼻锥各一条非空网格；与 OCC 网格 p95 抽检
- **依赖**：B2（圆柱是退化 snout）

### B5 `gm_CreateCircularTorus(double,double,double,double) → id`

- **映射**：`PrimCTorus`。圆管环：小圆 `CrossSection::circle` + `Manifold::revolve`。四参数（主半径、管半径、扫角、…）必须从调用点钉。
- **门**：整环 / 扇环非空；扫角 0 hard fail
- **依赖**：WP-C 的 revolve 语义（180° 半圆组合只属于扫掠截面，圆环面不一定拆）

### B6 `gm_CreateRectangularTorus(double×5) → id`

- **映射**：`PrimRTorus`。矩形截面绕轴旋转：矩形 `CrossSection` + `revolve`。
- **门**：同 B5
- **依赖**：B5 的参数钉法可复用

### B7 碟：`gm_CreateSphericalDish(double,double)` / `gm_CreateEllipticalDish(double,double,double)`

- **映射**：`PrimDish`。半椭圆旋转体：半椭圆折线 `CrossSection` + `revolve(180)` 或球面剖分。
- **门**：球碟 / 椭圆碟非空；高度 ≥ 半径时的扁碟
- **依赖**：B5 的 revolve 路径

### B8 `gm_CreatePyramid(double×7)` / `gm_CreateSlopeEndedCylinder(double×6)`

- **映射**：`PrimPyramid` / `PrimLPyramid` / 切角 `PrimSCylinder`。棱锥：底多边形 + 顶点凸包（`Manifold::hull`）或缩尺挤出。斜端柱：两端斜切，接近扫掠体的 Ruled 退化，优先在钉参数后再选 hull 或 ruled。
- **IDA 下一刀**：`0x1073c12c` 邻近的 pyramid / slope 工厂。
- **门**：正四棱锥、斜端柱非空
- **依赖**：B4；切角柱可能依赖 WP-C Ruled

### B9 `gm_CreatePolyhedron` + `gm_AddVertexToPolyhedron` + `gm_AddFacetToPolyhedron` + `gm_AddSideToFacetOfPolyhedron`

- **映射**：`PrimPolyhedron` → 顶点/面直接 `Manifold::from_mesh_f64`。非法网格 hard fail（与空挤出同一纪律）。
- **门**：四面体夹具；非流形面报错不写盘
- **并行**：可与 B3 同时

### B10 `gm_CreateNull` / `gm_CreateMark` / `gm_CreateStraight` / `gm_CreateArc` / `gm_CreateBezier`

- **映射**：辅助几何，不进 `PlantMesh` 生产路径。调用时响亮跳过或标 `bad`，禁止空网格充数。
- **门**：源码断言生产 `tessellate_libgm_param` 不含这些分支的成功路径

---

## WP-C  扫掠体（`DB_Gensec` → 三支 libgm）

权威函数：`do_solid_segments` `0x10732FF0`；逐段 `0x107318E0`。

### C0 截面：`gm_CreateProfile(D2_Profile)` / `gm_CreateProfile(D2_Point)` + `gm_AddSpan` / `gm_AddEndSpan` / `gm_AddCurve`

- **映射**：SANN / SPRO / SREC → `CrossSection`（多边形环，`FillRule::Positive`）。弧段先折线逼近（对标 `gm_SetFacetTolerance` / 圆分段）。
- **文件**：aios-core `prim_geo/profile` 或 gen-model `manifold_tessellate.rs` 的 profile 转换
- **门**：矩形、环形（含孔）、360° SANN 截面非空
- **依赖**：WP-A。**C1/C2/C3 都依赖 C0**

### C1 `gm_CreateExtrusion(unsigned profile, double height) → id`

- **映射**：直线且无斜切 → `Manifold::extrude(section, height)`。已有任意 `PrimExtrusion` 骨架。
- **门**：`square_extrusion_is_non_empty`、`empty_extrusion_is_hard_fail`（已绿）；直墙 RVM
- **依赖**：C0（目录截面）；WP-A 的挤出可先用 `verts`

### C2 `gm_CreateRevolution(double,double,D2_Point,double degrees, unsigned profile) → id`

- **映射**：`SpineArc` 段。实测调用 `gm_CreateRevolution(0.0, …, 180.0, profile)` 再 `gm_AddCurve` 拼半圆。360° SANN **必须**两半合并对拍，不得单次 360° 换拓扑（FR-006）。
- **文件**：`manifold_tessellate.rs` 或 `sweep_mesh.rs`
- **门**：弧墙 RVM；180+180 与参考网格体积差门
- **依赖**：C0

### C3 `gm_CreateRuledSolid(double len, unsigned profA, unsigned profB) → id`

- **映射**：两端截面不同（或斜切导致两端轮廓变了）。manifold 无现成 loft：两端 `CrossSection` 折线点列一一对应，侧面四边形拆三角，再 `from_mesh_f64`。点数不一致必须失败，不许静默重采样藏错。
- **门**：斜切墙 RVM；DRNS/DRNE 相对切向垂直/平行时不得误走 C3（ADR-026）
- **依赖**：C0、ADR-026 斜切平面谓词（已有纯函数）

### C4 斜切延伸（不是新的 gm_Create）

- **映射**：`0x107318E0` 日志 `Start-mitre extension` / `End-mitre extension`：再 `gm_CreateExtrusion` + `gm_CreateTransform` + `gm_AddMember`。即 C1 实体挂到段 CSG 树上。
- **IDA 下一刀**：把 `0x107318E0` 从 n≈420 到结束的延伸长度/方向反完，写成与 `set_mitre_planes` 配套的纯函数。
- **门**：带斜切平面的直墙 RVM；延伸长度为 0 时与 C1 网格一致
- **依赖**：C1、C3、`setMitrePlanes` `0x107368A0`（语义已在 ADR-026）

### C5 多段组树：`gm_CreateTransform` / `gm_SetTransform` / `gm_ShiftTransform` / `gm_RotateTransform` / `gm_AddMember`

- **映射**：`Manifold` 上 `translate` / `rotate` / `transform`，多段 `batch_union` 或 `compose`。
- **门**：两段 SPINE 的 GENSEC（若活库有）RVM；单段不走组树
- **依赖**：C1–C4。多段 SPINE 本期可后置（ADR-026 已声明不在当时范围）

---

## WP-D  CSG 树（对应 `gm_CreateCombination`）

### D1 `gm_CreateCombination(GM_Operation) → id` + `gm_AddMember`

- **映射**：已在 ADR-029：`batch_union` / `batch_difference`。Core3D 实测 `getCSGTree` 用 op `0` 建组合、负体分支 op `3`。enum 数值必须再钉（疑 UNION=0、DIFFERENCE=3）。
- **文件**：`manifold_bool.rs` / `manifold_csg.rs`
- **门**：GWALL extra `17496_116569` p95≤180；空差集不覆盖 `booled_id`
- **依赖**：WP-A 网格非空。与 WP-B 并行

### D2 `gm_CreateSection` / `gm_CreateCutSurface` / `gm_CreateClippedTree` / `gm_CreateExpandedTree` / `gm_CompressTree` / `gm_CreateSolidTree` / `gm_CreateNormalisedItem`

- **映射**：裁剪/展开/压缩是 libgm 树优化，不是新图元。生成管线用 manifold 的布尔结果即可，**不复刻这些 API**。
- **门**：源码断言生产路径不调用；若浸水/开孔日后需要 section，另开规格
- **依赖**：无（明确不做）

### D3 `CSG_TreeBuilderCat::addNegatives` / `CSG_PrimitiveUtilities`

- **映射**：目录负体已有 `apply_cata_neg_boolean_manifold`。保持，不改入口。
- **门**：现有目录负体源码顺序测试
- **依赖**：D1

---

## WP-E  查询与离散（只做生成需要的）

| 符号 | 方案 |
|---|---|
| `gm_QueryFacetData` / `gm_SetFacetTolerance` / `gm_SetDefaultFacetTolerance` | `to_mesh_f64` + 圆分段/`circular_segments` 对标公差。不调 libgm |
| `gm_QueryLimits` | `PlantMesh.aabb` / manifold bounding box |
| `gm_QueryProfile` / `gm_QuerySpan` / `gm_QueryItem` / `gm_QueryType` | 调试用，生产不需要 |
| `gm_QueryEdgeData` | OCC 路径现在用边中点写 `pts`；manifold 路径改用网格 AABB 角点或显式失败，禁止静默空 `pts` |
| `gm_ValidateObject` / `gm_ValidateTree` | `Manifold::status` / `is_empty`；失败 hard fail |
| `gm_CreateIterator` 族 | 不移植；Rust 直接持有 `Vec<Manifold>` |
| `gm_QueryClash` / `gm_QueryClose` / `gm_QueryXRay` / `gm_QueryMass` / `gm_Picture*` | **不做**（碰撞/出图） |
| `gm_CreateBody` / `AM_CoEdge::partner` | **不做**（B-rep 内核） |

---

## 推荐实施顺序

```
WP-A 接生产回退
  ├─ D1 布尔（ADR-029，可并行）
  ├─ B3 球
  ├─ B9 多面体
  └─ C0 截面
       ├─ C1 挤出（目录截面版）
       ├─ B4 Snout（先钉参数）
       ├─ B5/B6/B7 旋转类
       └─ C2 扫掠旋转 → C3 Ruled → C4 斜切延伸 → C5 多段
            └─ B8 棱锥/斜端柱（可能借用 C3）
WP-E 边点/AABB 口径
Phase 4 删 occ（规格 FR-008 盘点通过之后）
```

## 验证

- 每包：纯函数单测（不连库）。空输入必须红。
- 扫掠体：直墙 / 斜切墙 / 弧墙 / 360° SANN 走既有 RVM 门，不放宽阈值。
- 布尔：`pe:17496_116569` p95≤180。
- `cargo fmt`、`cargo check`；禁止 `cargo clean`。
- live 更新 `docs/2026-08-12_live-test-ledger.md`。
- 本地 vendor patch 不得推 main；aios-core `gen_model` 与 `manifold-sys` 解耦要先合上游。

## 明确不做

- 通用 3D 脊线扫掠（超出 C1–C3）。
- OCC 布尔回生产。
- 把 `gen_manifold_mesh` 只放在 gen-model 长期不回 aios-core。
- 复刻 libgm handle / iterator / picture / clash。
