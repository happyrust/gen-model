# Tasks：按 libgm 原语用 manifold-csg 替换 OCC

**Input**：`specs/009-retire-occ/plan.md`（2026-08-23 重写）
**Prerequisites**：ADR-030（含两版 IDA 修订）、ADR-044、ADR-026、ADR-029；
宪法 I–III（响亮失败、单一规则）

> 勾选表示**本工作树已落地代码**。aios-core 与 `manifold-sys` 解耦目前只在本地 vendor +
> `Toggle-LocalDeps -On`，**不能推 main**，合上游前相关项视为未完成发布。
>
> **2026-08-23 校准**：T009 / T012 / T014 / T015 / T016 的实现早就进了
> `tessellate_libgm_param`，但清单一直没勾；更要紧的是它们的 IDA 前置（T011 / T013 与
> T016 的钉参数那一半）**被跳过了**——参数顺序至今没跟 libgm 核对过。下面把「接线」与
> 「钉参数」拆成两条，免得再靠一个勾同时代表两件事。

## 流程与底座

- [x] T001（串行）`docs/adr/ADR-030-retire-occ-tessellation.md`、`specs/009-retire-occ/spec.md`、
  `specs/009-retire-occ/plan.md`：ADR → spec → plan；权威定为 libgm `gm_Create*` 而非 OCC。
- [x] T002（串行）`Cargo.toml`、`../vendor/old-aios-core/Cargo.toml`、
  `../vendor/old-aios-core/src/csg/mod.rs`、`../vendor/old-aios-core/src/prim_geo/extrusion.rs`、
  `../vendor/old-aios-core/src/prim_geo/revolution.rs`：
  `gen_model` 不再捆绑 `manifold-sys`；本仓 `manifold` 只拉 `manifold-csg`。发布前升 aios-core rev。
- [x] T003（串行）`src/fast_model/manifold_csg.rs`、`src/fast_model/manifold_bool.rs`：
  布尔 ingest f64、空网格 hard fail、禁止 `ManifoldRust`（ADR-029 骨架）。
- [ ] T004（串行，依赖 T003）编译并跑 `cube_minus_inner_cube` / `ingest_rejects_empty_mesh`；
  live `mesh_gwall_extra_against_cwall`；更新 `docs/2026-08-12_live-test-ledger.md`。
- [x] T005（串行，依赖 T003）`src/fast_model/mesh_generate.rs`：`not(occ)` 且无
  `tessellate_libgm_param` 后端时 `bail!`，禁止 `Ok(())` 静默跳过。

## WP-A / B1–B2 箱柱挤出骨架

- [x] T006（可与 T003 并行）`src/fast_model/manifold_tessellate.rs`、`src/fast_model/mod.rs`：
  单位箱、单位柱、`PrimExtrusion` 轮廓挤出；空挤出 hard fail。五条单测已绿。
- [x] T007（串行，依赖 T005+T006）`src/fast_model/mesh_generate.rs`：`gen_inst_meshes` 对
  `PrimBox` / `PrimLCylinder` / 无切角 `PrimSCylinder` / `PrimExtrusion` 先
  `tessellate_libgm_param`，失败或 `None` 再 OCC；AABB/`pts` 改从网格取，禁止空 `pts` 静默。
- [x] T007a（2026-08-19 补口径）`src/fast_model/manifold_tessellate.rs`：挤出 FRADIUS 倒角接
  `gen_polyline_original` 权威离散（弦高容差 `Extrusion::tol()`，体积对拍 1% 钉住）。
  ~~样条轮廓（`CurveType::Spline`）回退 OCC，不得折线近似。~~
  **2026-08-23 推翻**：该变体在整个工作区无构造点，且它不是样条而是弧形墙截面。转 T036。
- [x] T007b（2026-08-20 补口径）`src/fast_model/manifold_tessellate.rs`：`PrimRevolution`（REVO/NREV）→
  `tessellate_revolution`，与挤出共用 `flatten_profile_loop` 的倒角离散。
  ~~出平面的回转轴仍回 `None` 走 OCC。~~ **2026-08-23 推翻**：libgm 的回转轴是平面内
  角度，出平面轴表达不出，应 `bail!`。转 T033。共用挤出离散这件事本身也是错的，转 T040。
- [ ] T008（依赖 T007）抽检 live：一箱、一柱、一挤出网格非空；台账一行。

## WP-B 其余目录原语

- [x] T009 `src/fast_model/manifold_tessellate.rs`：`gm_CreateSphere` → `unit_sphere()`（半径 0.5）。
  **段数写死 16×36，未按真实半径算**；球走单位几何，被身份键挡着，转 T038 + T041。
- [x] T010 `src/fast_model/manifold_tessellate.rs`：`PrimPolyhedron` → 逐面 earcut 剖分 + 有向体积
  定朝向；一张面都剖不出来 hard fail。**改了口径**：plan 原写 `from_mesh_f64`，但面片壳
  不做 CSG，走 manifold ingest 只会把非水密的现场面片挡在渲染之外，而 OCC 那边
  (`Polyhedron::gen_occ_shape`) 也只是 `Shell::from_faces`，同样不保证实体。五条单测。
- [x] T011（2026-08-24 完成）IDA `idalib-18608`：钉 `gm_CreateSnout` 五参数顺序。
  **`gm_CreateSnout(rBtm, rTop, height, xShift, yShift)`**，与 `gen_snout` 逐位一致。
  两头对照：libgm `0x100392A0` → `GM_Snout::GM_Snout`（`0x1002FCE0`）把五个实参依次写进
  `this+5..+9`，而 `getBottomRadius`/`getTopRadius`/`getHeight`/`getXShift`/`getYShift`
  （`0x1002FB30`…`0x1002FB70`）正好读这五格；Core3D `CSG_BasicSNO::getPrimGeom`
  （`0x10727450`）传 `(DBOT*0.5, DTOP*0.5, HEIG, XOFF, YOFF)`，`CSG_BasicCON`
  （`0x10726B30`）走同一函数、两个偏移填 0。
  **顺带查出两处形状缺陷，见 T050 / T051——参数顺序对，几何摆位不对。**
- [x] T012 `src/fast_model/manifold_tessellate.rs`：`PrimLSnout` → `gen_snout` 锥台 ± 偏心。
  段数写死 32 且未取 `fmax(rBtm, rTop)`，转 T038。
- [x] T013（2026-08-24 完成）IDA：钉 `gm_CreateCircularTorus` / `RectangularTorus` 参数。
  **`gm_CreateCircularTorus(rIns, rOut, startAngle, finishAngle)`**（`0x10039A00` →
  `GM_CircTorus` 字段 `this+5..+8`，`getRInner`/`getROuter`/`getStartAngle`/`getFinishAngle`）；
  **`gm_CreateRectangularTorus(rIns, rOut, height, startAngle, finishAngle)`**
  （`0x100397F0` → `this+5..+9`）。
  Core3D `CSG_BasicCTO`（`0x10726BE0`）/ `CSG_BasicRTO`（`0x10727140`）的
  **startAngle 恒为 `0.0`**，finishAngle = `ANGL`——所以本仓用单个 `sweep_deg` 表达是等价的。
  角度单位是**度**（同一族的 `GM_SlopeEndCyl::validate` 直接与 `90.0` 比较）。
  另一条现成欠项：两个 Core3D 入口都对内半径做 `fmax(RINS, 0.0)`，本仓 `check_valid` 未夹。
- [x] T014 `src/fast_model/manifold_tessellate.rs`：圆环面 / 矩形环面 → `gen_circular_torus` /
  `gen_rectangular_torus`。段数写死且两方向半径喂错，转 T038。
- [x] T015 `src/fast_model/manifold_tessellate.rs`：`PrimDish` → 球碟 / 椭圆碟。两套段数公式都没抄，转 T038。
- [x] T016（2026-08-24 完成）IDA 钉 `gm_CreatePyramid` / `gm_CreateSlopeEndedCylinder` 参数顺序。
  **`gm_CreatePyramid(xBot, yBot, xTop, yTop, height, xShift, yShift)`**（`0x10039030` →
  `GM_Pyramid` `this+5..+11`，七个 getter 一一对上）；Core3D `CSG_BasicPYR`（`0x10726F90`）
  按 `XBOT/YBOT/XTOP/YTOP/HEIG/XOFF/YOFF` 原序传入。`GM_Pyramid::calcRange`（`0x10094980`）
  的支撑函数是 `(xShift·dx + yShift·dy + height·dz)/2`，确认**偏移上下各摊一半**、
  `xBot` 是全宽——`gen_pyramid` 两条都写对了。
  **`gm_CreateSlopeEndedCylinder(radius, height, xBase, yBase, xTop, yTop)`**：wrapper
  （`0x100394B0`）把 `(a3,a4)` 与 `(a5,a6)` **换位**后才喂 `GM_SlopeEndCyl::GM_SlopeEndCyl`
  （`0x10030180`，字段序是 radius/height/xTop/yTop/xBase/yBase），所以对外签名是**底面在前**，
  与 `gen_slope_ended_cylinder(r, h, btm_angles, top_angles, …)` 一致。
  角度是度且 `validate`（`0x10030300`）要求严格落在 (−90, 90)；Core3D（`0x107272D0`）
  先把 `XTSH/YTSH/XBSH/YBSH` 逐个折进该区间（>90 减 180，<−90 加 180），**本仓没做这一步**，
  已知欠项。
- [x] T017（2026-08-24 完成）源码断言：`gm_CreateNull` / Mark / Straight / Arc / Bezier 不得作为
  `tessellate_libgm_param` 的成功分支。落为 `the_curve_primitives_are_not_shape_arms`
  两道闸：五个曲线图元的名字不许出现在生产半区（名字先落进来，分支就是下一步）；
  分发臂集合钉死为 14 形状 + `Unknown`/`CompoundShape`，`PdmsGeoParam` 新变体
  必须先过这份清单，届时「实体还是曲线」得当面回答。依据 ADR-030 IDA 修订二：
  这五个走 `calcFacetsWithoutSurfaces` 出折线、靠 `gm_AddCurve` 挂树，不产实体。

## WP-C 扫掠体

> 截面与成体不走 manifold-csg：自建网格，manifold 只用于布尔（与 WP-B 同一决定）。
> 内核落在 `src/fast_model/sweep_mesh.rs`，纯函数单测已绿。
>
> **2026-08-20 口径改动**：`PrimLoft` 已接进 `tessellate_libgm_param`（`sweep_solid_mesh`
> 三支齐上），不再等 RVM 门。理由是扫掠体是活库里数量最大的一类，不接则 `occ` 无从退役；
> 代价是 T019/T020/T022 的 RVM 对拍**变成了事后验收**——它们仍未完成。

- [x] T018（串行）截面：SANN / SPRO / SREC → 2D 闭合环（外环逆时针 + 孔顺时针，弧折线）。
  文件：`src/fast_model/sweep_mesh.rs`。倒角与弧段复用 aios-core `wire::gen_polyline_original`，
  弧折线化改走 `libgm_discretise::span_polyline_by_tol`（libgm 的整圆角度格子），
  端盖三角剖分用 earcutr（凹截面必须）。
- [ ] T019（依赖 T018）目录截面版 `gm_CreateExtrusion`；直墙 RVM 门。
  内核 `extrude_loops` / `loft_loops` 已绿；生产接线已落地；**缺 RVM**。
- [ ] T020（依赖 T018）`gm_CreateRevolution(..., 180°)` 两半合并；弧墙 + 360° SANN 体积门（FR-006）。
  内核 `revolve_loops` 已绿；**缺弧墙 RVM**。
- [x] T021（2026-08-24 完成）IDA 反编译 `0x107318E0` 斜切延伸段（Start/End-mitre extension），
  写成纯函数。落在 `src/fast_model/sweep_mesh.rs`：`mitre_extension_reach` +
  `mitre_extension_length`。
  `sub_107318E0` 就是扫掠段构建器（日志字符串 `Start-mitre extension:` /
  `End-mitre extension:` 在里面），三支 `gm_CreateExtrusion` / `gm_CreateRevolution` /
  `gm_CreateRuledSolid` 齐全。延伸长度的算法在它调的 `sub_10733720`（`0x10733720`）：
  ```text
  |plane_dir.z| ≤ 1e-6                  → 0（切面与扫掠方向平行，不用延伸）
  denom = dot(plane_dir, line_dir)
  每个采样点 p:  z = |denom| > 1e-6 ? dot(p, plane_dir) / denom : 0
  采样点 = 轮廓每个顶点 + 每条 |bulge| > 1e-6 的弧上 9 个内点
  reach = max(|z_max|, |z_min|)；出参另给 包围盒对角线 × 2.2
  extra = reach > 1.0 ? reach + 1.0 : reach     ← 是 +1，不是 ×2 也不是按比例
  total = 端点间距 + extra
  ```
  **那 9 个点是第四套离散口径**（挤出格子、回转配对、曲面原语段数之外），与容差无关，
  只服务这个包围盒，别拿去铺三角——已收成 `MITRE_ARC_SAMPLES` 并在文档里点名。
  **一处诚实的不确定**：循环上界 9 是从反汇编读到的，但 `evaluatePoint` 的实参被
  Hex-Rays 吞了，参数化按 `t = k/10` 均分推断实现。取密一点会让 `reach` 偏大，
  所以宁可照抄 9，不要「反正更细更安全」——这条写进了函数文档。
  门（四条全绿）：切面平行时回 0；45° / 60° 两组手算 reach（30 / 30√3）；
  半圆的极值只在弧腰上，抹平 bulge 后归零（漏采弧就红）；`+1` 的边界
  （reach = 1.0 不加、2.0 → 3.0）。`sweep_mesh` 28 全绿。
  **2026-08-24 已接生产**：T023 的 RuledSolid 路径按 reach 延伸后通过 Manifold
  `trim_by_plane` 裁两端；45°/60°、平行切向与既有分派单测全绿。现场门仍依赖 WP-J。
  另外这一趟顺带给 F1 添了一条旁证：弧段走
  `gm_CreateRevolution(0.0, sweepDeg, pAxis, 180.0, profile)`——轴角是个写死 180 的
  **平面内角度**，出平面轴确实表达不出。
- [ ] T022（依赖 T018；**不依赖 T021**）`gm_CreateRuledSolid`：两端轮廓一一对应连三角。
  内核 `loft_loops` 已绿（斜切不改体积）；**缺斜切墙 RVM**。
- [ ] T023（依赖 T022）斜切延伸挤出挂到段 CSG；斜切墙 RVM；垂直/平行切向不得误走 Ruled（ADR-026）。
  **代码完成、RVM 未完成**：生产 RuledSolid 先按 `mitre_extension_length` 延伸，随后按
  `working_mitre_plane` 裁切；只由权威分派进入，垂直/平行仍走 Extrusion。
- [ ] T024（可后置）多段 SPINE：`transform` + `batch_union`；无多段夹具则只单测。

## WP-D CSG 树

- [ ] T025（依赖 T003）布尔 live：`mesh_gwall_extra_against_cwall` p95≤180；
  空差集不覆盖 `booled_id`（**静态半 2026-08-19 已落地**：设计/目录两条 manifold
  生产路径均在写盘前拦空差集 → `bad_bool` 出声，
  `empty_difference_is_bad_bool_not_a_silent_swallow` 钉住；live p95 未跑）。
- [x] T026（可并行）源码断言生产路径不调用 Clip/Expand/Compress/SolidTree/Picture/Clash。
- [x] T027（依赖 T025）确认 `apply_cata_neg_boolean_manifold` 仍是目录负体唯一入口。

## WP-E 离散口径与收口

- [x] T028（依赖 T007）`src/fast_model/mesh_generate.rs`：manifold 路径 AABB 来自 `PlantMesh`；`pts` 有明确
  来源或省略策略写进回执，禁止空列表假装成功。
> T029（活库盘点）已拆分为 T045；T030（测试与浸水解绑）已拆分为 T043 / T044。
> 两条不再单独跟踪，勾在拆出去的那几条上。

- [ ] T031（依赖 WP-F + WP-G + WP-H + WP-J 全绿）`Cargo.toml` default/release 去掉 `occ`；
  CI 增加无 occ 的 `gen_inst_meshes` 失败闭合测。
- [ ] T032（串行，收尾）`changelog.md`、live 台账、`cargo fmt` / `cargo check`；
  aios-core 解耦合上游并 `Toggle-LocalDeps -Off` 后再推。

---

## WP-F 假回退收口（ADR-030 决策 8）

做完之后 `tessellate_libgm_param` 里不再有任何「回退 OCC」语义的 `Ok(None)`。

- [x] T033（串行）`src/fast_model/manifold_tessellate.rs`：出平面回转轴由
  `return Ok(None)` 改 `bail!`，错误带上实际 `rot_dir`。改
  `out_of_plane_revolution_axis_falls_back_to_occ` 为「必须报错」，并加一条源码顺序断言：
  `tessellate_revolution` 函数体内不得出现 `Ok(None)`。
  依据：`GM_Revolution` 构造 `0x10033830` 的轴是 `D2_Point` + `axisAngle`；
  本仓 `Revolution` 唯一构造点走 `Default`，`rot_dir` 恒为 `Vec3::X`。
  **2026-08-23 落地**：`an_out_of_plane_revolution_axis_is_a_hard_error` +
  `none_is_only_the_not_a_shape_verdict`（生产半区 `Ok(None)` 恰一处且在非形状臂，
  比逐函数断言更严）。
- [x] T034（依赖 T033）**IDA 下一刀**：钉 `GM_User::normtol_` 的初始化取值
  （与 `arctol_` 同一处），写进 `libgm_discretise` 模块文档。
  **2026-08-23 落地**：`normtol_ = 1e-6`（libgm 3.1 `0x10109020`，与 `arctol_`
  `0x10109028` 相邻）；写入器在 libgm 内零调用、Core3D 未导入该符号——运行期恒为
  初值。落为 `libgm_discretise::NORM_TOL`，出处写进常量文档。
- [x] T035（依赖 T034）`src/fast_model/manifold_tessellate.rs`：`tessellate_revolution` 在标准位下补
  `movePointsOntoYAxis`（`0x100978A0`）的轴心吸附。门：轮廓贴轴时回转后轴心无针状面，
  体积对解析值 1%。**这是新欠项，不是等价改写。**
  **2026-08-23 落地**：半径坐标 `|r| < NORM_TOL` 精确置 0；
  `a_profile_hugging_the_axis_is_snapped_onto_it` 钉住（5e-7 噪声轮廓：体积对
  π·R²·L、顶点半径不得落在 (0, 1e-4) 开区间、轴上必须真有顶点）。
- [x] T036（可与 T033 并行）`src/fast_model/manifold_tessellate.rs`：`CurveType::Spline` 分支实现为
  弧形墙截面——三点定圆 → 两条 bulge 弧 → `libgm_discretise::span_polyline_by_tol`
  离散 → 闭环挤出。**不新写弧数学。** 门：环形扇区体积对帕普斯（1%）；点数不等于 3
  hard fail；三点共线 hard fail。删除 `spline_extrusion_falls_back_to_occ`。
  **2026-08-23 落地**：`tessellate_arc_wall`（三点圆心复用 aios-core
  `cal_circus_center`，内外圈同一套 `gen_occ_spline_wire` 点位，直线挤出与弧形墙
  共用 `extrude_flat_polygons` 尾段）；三条门测试 +「SPINE 点出平面 / thick 吃穿
  半径」也硬失败。
- [x] T037（可并行）`src/fast_model/manifold_tessellate.rs`、`src/fast_model/mesh_generate.rs`：`Unknown` /
  `CompoundShape` 直接推进 `unbuildable` 标 `bad`，不经 OCC 分支
  （两者 `check_valid()` 为 false，`gen_occ_shape()` 本来就 `Err`）。
  门：源码断言 manifold 分支之后不再有 `#[cfg(feature = "occ")]` 的形状回退路径。
  **2026-08-23 落地**：`gen_inst_meshes` 只剩 manifold 一台形状引擎（`None`=非形状、
  报错=坏参数，一律标 `bad`）；OCC 的 `shapes_map`/`gen_occ_shape` 回退整段拆除，
  后端门改为 `not(manifold)` 即 bail；`gen_inst_meshes_has_no_occ_shape_fallback` 钉住。

## WP-G 离散口径对齐（ADR-044；Phase 4 硬前置）

- [x] T038（2026-08-23 完成）`src/fast_model/libgm_discretise.rs`、
  `src/fast_model/manifold_tessellate.rs`、`src/fast_model/mesh_primitives.rs`：
  曲面原语段数改由真实半径算。规则各自成一个纯函数，doc 里带 §7.9.1 的调用点地址：
  `cylinder_segments`（`0x100532F0` / `0x1009DFC0` / `0x100A20F0`）、
  `snout_segments`（`0x1009EA30`，`fmax(rBtm, rTop)`）、
  `torus_ring_segments`（`0x10047150` / `0x100962F0`，喂 **rOut** 走 `partRev`）、
  `circular_torus_tube_segments`（管截面喂 `(rOut−rIns)/2`）、
  `spherical_dish_facets`（`0x10099CF0`，绕轴 `h ≥ a ? R : a`、经向沿用绕轴角步长）、
  `elliptical_dish_around_segments`（`0x10054AB0`，只做了绕轴那一半）。
  `gen_spherical_dish` / `gen_elliptical_dish` 的签名从「一个 `segments`」改成
  「`slices` + `stacks`」，两个方向都由调用方按权威规则给，生成器内部不再自造
  `segments/2`。**libgm 的「非整圈 +1」不在这里加**——环面生成器内部已经做了
  （`ring_count = ring_segments + 1`），doc 里点名了，别加第二次。
  门：8 条对照表单测，期望值**手算自 §7.9.1**（含「深碟极角必须是钝角，不许回到
  `asin`」「两个方向拿到同一个数就说明喂错了半径」两条判别性断言）。
  `libgm_discretise` 25 / `mesh_primitives` 23 / `manifold_tessellate` 29 全绿，
  `cargo fmt` + `cargo check` 通过。
- [x] T038a（2026-08-24 完成）**椭圆碟不是段数欠项，是形状欠项——生成器整个换掉了。**

  落地：`libgm_discretise` 新增 `EllipticalDishFacets` + `elliptical_dish_facets`
  （形状三量 `r_k` / `R_c` / `θ` 与三个方向的段数一次解出，与 libgm 同一个函数里
  先算形状再按形状分段的结构对齐）；旧的 `elliptical_dish_around_segments` 与
  `elliptical_dish_meridional_segments` **一并删除**——后者正是这一条要还的具名欠账，
  留着就是留一个「看着像规则」的错值。
  `mesh_primitives` 侧新增具名结构体 `TorisphericalArc`，`gen_elliptical_dish`
  改成 `(arc, slices, hub_stacks, knuckle_stacks)`，母线换成球冠 + 相切环面拐角。
  **参数用结构体不用五个位置参数**：`base_radius` / `hub_radius` / `knuckle_radius`
  三个都像「半径」，正是 T011 那一轮反复核对的顺序陷阱。
  分发臂的源码顺序断言表跟着从 `&[2, 3]` 改成 `&[1, 2, 3]`。

  门（四条全绿）：(a) `elliptical_dish_is_a_torispherical_head_not_an_ellipsoid`
  ——体积对托里球形封头的解析值 2%；(b) `the_two_arcs_meet_smoothly_at_the_transition_angle`
  ——两段母线在 θ 处位置重合，且 θ 必须是 26.565°（抄错公式那条给 83.9°）；
  (c) `a_hemispherical_torispherical_head_matches_the_spherical_dish`；
  (d) `libgm_discretise` 侧两条对照表 + 一条退化。
  `mesh_primitives` 26 / `libgm_discretise` 35 全绿；全量 `--lib` **1073 通过 / 1 失败**
  （那一条仍是 vendor 未发布问题）。`cargo fmt` + `cargo check` 通过。

  **写门时发现体积这个判据本身很钝，值得记一笔。** 第一版取 a=2 / h=1.5，托里球形
  封头与旋转椭球的体积只差 **0.6%**——比网格自身的离散误差还小，那组尺寸下这条测试
  **换不换对曲面都会绿**。是自检断言「两族必须分得开」把它红出来的，不是主断言。
  改成浅碟 a=10 / h=1 后差 16%。另外体积基准的球冠项 `R_c³·(2/3 − cosθ + cos³θ/3)`
  是「大数乘极小差」（8.8e5 × 3e-5），f32 只剩两位有效数字，测试辅助函数一律走 f64
  ——否则基准会先于被测对象失真。

  原始欠项描述与规则出处保留在下面。
  ~~椭圆碟的经向还不是权威值；IDA 下一刀反编译 `knuckleRadiusToUse` / `radiusOfHub`，
  反完之后换的就是 `elliptical_dish_meridional_segments` 那一个函数体。~~
  **2026-08-24 那一刀已下**（libgm `GM_EDish::calcFacetsWithoutSurfaces` `0x10054AB0`、
  `knuckleRadiusToUse` `0x100556A0`、`radiusOfHub` `0x10055750`、`isSpherical` `0x100313E0`；
  Core3D `CSG_BasicDIS::getPrimGeom` `0x10726D10`），结论比原以为的大一档：
  **libgm 的椭圆碟是托里球形封头——球冠 + 环面拐角、两段相切；本仓
  `gen_elliptical_dish` 画的是半个旋转椭球。** a=2 / h=1 时径向差 1%–1.2%，而且母线的
  环带划分方式根本不同；`cancelFacets` 只消全等重叠（§6.11），这种情况下共面抵消一条都
  不会生效。**只换 `elliptical_dish_meridional_segments` 的函数体是补不上的**，
  `mesh_primitives::gen_elliptical_dish` 与它的段数入参一起重做。

  权威规则（`a = DIAM/2` 即 `getBaseRadius`，`h = HEIG`，`tol = FACET_TOL_MM`）：

  1. **拐角半径 `r_k`**。Core3D 不用用户填的 `RADI`——`RADI` 只当「椭圆碟 / 球碟」的开关
     （`> 0` 走椭圆），数值被丢掉，实参是现算的
     `r_k = h / (1 + (a − h)/√(a² + h²))`。本仓现状恰好也只拿 `d.prad > 0.0` 当判据，
     这一半是对的，**别有人「顺手修成用 prad 当拐角半径」**。
     `knuckleRadiusToUse()` 在 E3D 给的这个入参下三个分支落到同一值，等于恒等返回，
     所以不必复刻那段选择逻辑——但要在注释里写明「为什么可以不复刻」。
  2. **球冠半径 `R_c` = `radiusOfHub()` = `(a² + h² − 2a·r_k) / (2(h − r_k))`**，
     闭式化简 `R_c = s(s + a − h) / (2h)`，`s = √(a² + h²)`。这正是过顶点 `(0, h)`、
     与拐角环面内切的那个球冠半径（按切点条件独立验算通过）。球心在轴上 `z = h − R_c`；
     拐角环的管心圆半径 `a − r_k`、位于 `z = 0`，管半径 `r_k`。
  3. **交接角 `θ`**。~~`θ = acos((h − r_k)/(R_c − r_k))`~~ ——**上一版记的这条是错的**，
     它来自 Hex-Rays 把 acos 实参吞掉之后的伪码。看反汇编（`0x10054CCB`）是
     `acos(1 − q)`，`q = (h − r_k)/(R_c − r_k)`；小 `q` 分支取 `sqrt(2q)`，恰是
     `acos(1 − q)` 的小角展开，两边自洽。化简后
     **`θ = acos((R_c − h)/(R_c − r_k)) = atan2(h, a)`**。
     按错的那条实现会得到一个带折痕的碟（a=2 / h=1 时 83.9° 对正确的 26.6°）。
     `isSpherical()`（`|a − h| ≤ 1e-6`）时直接取 `π/4`，且此时 `R_c` 保持等于 `r_k`。
  4. **段数**。绕轴 `n_around = circle(a, tol)`——**喂底半径 `a`，不是 `R_c`**；超 1000
     是直接截断（曲面原语这条路确实是硬截断，与 T040 里轮廓那条路的整体重标定不是一回事）。
     经向拆两段：`n_hub = partRev(R_c, tol, 0°, θ°)`、`n_knuckle = partRev(r_k, tol, θ°, 90°)`；
     封顶判据 `2(n_hub + n_knuckle) > 1000` 触发后，`4·n > 1000` 的那一段各自夹到 250
     （不是两段一起缩）。网格规模是「顶点 1 个 + `n_around × (n_hub + n_knuckle)`」。

  ~~**验收缺口**：两个盘点库是 `PrimDish` 库 A 17 行 / 库 B 0 行，且没拆球碟与椭圆碟，
  换完形状族能不能现场验收存疑。~~
  **2026-08-24 补测（库 A）：样本有，而且是多数。** 17 行里 **15 行是椭圆碟**
  （`prad > 0`）、2 行球碟，`geo_relate` 边 **102** 条。T038a 不是纯防御，改的是
  实打实会出现在现场的几何。RVM 抽检（T049）该有它一条。
  但顺着这一列查下去撞出一件更大的事，**已独立开为 T053**：碟（连同两种环面、
  不偏心的 Snout）在 `inst_geo` 里全是单位几何，权威段数规则拿到的是单位半径，
  所以 T038a **规则对了但现场还没生效**。证据
  `docs/evidence/2026-08-24-unit-normalised-curved-primitives.md`。
- [x] T039（2026-08-23 完成）`src/fast_model/mesh_primitives.rs`、
  `src/fast_model/manifold_tessellate.rs`：`DEFAULT_CIRCULAR_SEGMENTS` 已删，
  `unit_sphere` 的 `stacks` / `slices` 改由调用方给（与 T038 对两个碟生成器的改法一致）。
  柱与球剩下的写死段数**没有消失，也不该在这一步消失**——它们是单位网格身份键的欠账
  （G3 / T041），现收成 `manifold_tessellate` 里一处私有 `mod unit_mesh_identity`
  （`CYLINDER_SEGMENTS` / `SPHERE_STACKS` / `SPHERE_SLICES`），doc 里写明「按构造就是
  错的、值一位不许动、T041 换键时整组删」。**取值一位未改**：身份键不变而网格内容变了，
  等于在稳定 `geo_hash` 底下悄悄换几何。
  门：`the_generators_carry_no_default_segment_count`（`mesh_primitives` 生产半区不得
  再有 `const *SEGMENTS`）+ `every_segment_count_is_named_or_computed`（分发函数里
  段数只许来自 `libgm_discretise` 规则或那处具名欠账，`segs()` 实参不许是常数，
  用了 `segs()` 的臂必须自己取规则）。`mesh_primitives` 24 / `manifold_tessellate` 31 全绿。
  **2026-08-24 收口**：上面那两条都是按 `segs(` 反查，只看得见已经走了规则的那些，
  漏掉了椭圆碟经向的 `(around / 2).max(4)`——它混在 `d.pdia, d.pheig` 中间，既没进
  `segs(`，也没有任何东西说明它是 T038a 的欠账，改动它一位不会有测试变红。
  已收进具名 `libgm_discretise::elliptical_dish_meridional_segments`，并给
  `every_segment_count_is_named_or_computed` 补上**按生成器正查**的一段：9 个吃段数的
  调用点连同段数实参下标列成表，逐个实参判「不许出现裸数字」（`as i32` / `f64` 这类
  类型名里的数字前面挨着字母，不算），局部名（`around` / `meridional` / `*_segments`）
  走同一条判据。回退成 `(around / 2).max(4)` 即红，已实测。
- [x] T040（2026-08-23 完成）`src/fast_model/libgm_discretise.rs`、
  `src/fast_model/manifold_tessellate.rs`：新增回转 / collar 口径。
  `libgm_discretise` 侧三个新函数 + 一个常量：`paired_span`（`0x1008F7F0`）、
  `profile_steps`（`setNSteps` + 全局重标定）、`profile_steps_extruded`（挤出口径，
  并列存在只为让「两套确实不同」可测）、`PROFILE_FACET_CAP = 1000`。
  `manifold_tessellate` 侧拆成 `flatten_profile_loop`（挤出）与
  `flatten_profile_loop_revolved`（回转），共用 `profile_spans_of` 与 `assemble_ring`
  ——**格子函数是同一个 `span_polyline_in_steps`，不同的只有 `steps`**。
  `tessellate_revolution` 改用后者。
  已知窄于 libgm 一处：配对只在本环内找（libgm 的 `GM_Profile` 一个对象装整条轮廓
  含孔环）。要发生得让孔精确贴到外边界上，活库里没见过，症状是段数不一致而非静默变形，
  已写进函数文档。
  ~~前置 IDA 下一刀：反编译 `GM_Profile::pairedSpan`（`0x1008F7F0`）钉配对规则。~~
  **2026-08-23 已钉死**，三条规则全文见 plan 的 G1：配对 = 同两点反方向（精确浮点相等）；
  `n = circle(fmax(自身半径, 配对半径), tol)` 且与已存步数取大；`total > 1000` 时
  **清空数组 + `tol' = tol·((total−nSpans)/(1000−nSpans))²` 整条重算**，不是逐段截断。
  两条路最终都调 `getApproxPolyLineInSteps(n)`（本仓 `span_polyline_in_steps` 已是它），
  所以只需换段数计算，不需要新弧数学。
  门（**四条全绿**）：(a) `the_revolution_caliber_is_not_the_extrusion_one`——同一条透镜
  轮廓，挤出得 `[32, 40]`、回转得 `[40, 40]`，合并即红；
  (b) `paired_span_finds_the_same_two_points_walked_backwards`——含三角形无配对、
  退化段回 `None` 两个反例；
  (c) `an_over_dense_profile_is_rescaled_not_truncated`——R=10000 配 0.005 容差触发
  1573 点，重标定后落到 1000 以内**且单段步数仍远超 1000**（按逐段截断复刻的话这一条必红）；
  (d) `the_revolution_path_uses_the_paired_caliber`——源码顺序断言，回转退回挤出口径即红。
  另加 `a_sparse_profile_is_left_alone`（没超限不多算）。
  `libgm_discretise` 29 / `manifold_tessellate` 30 全绿，`cargo fmt` + 全量
  `--lib` 1058 通过（余 2 条红是既有的，见下）。
- [x] T040a（2026-08-23 完成，文档修订）两处错误口径已改：
  `plant-4/libgm-boolean-algorithm.md` —— §7.9.1 调用点表里 `setNSteps` 那行的第二个
  半径从「截面半径」改成「配对 span 半径」，「封顶是 1000」一段加上「只适用于曲面原语」
  的限定与告警框，并新增 **§7.9.2「轮廓那条路」**（`pairedSpan` / `setNSteps` /
  全局重标定 / 硬边负号，四条带伪代码）。
  `src/fast_model/libgm_discretise.rs` —— `MAX_SEGMENTS` 与 `circle_segments_uncapped`
  的文档注释补上「轮廓是第三种口径，本模块还没有，见 T040」，并点明
  `tessellate_revolution` 目前误用了挤出口径。
  `cargo fmt --check` 与 `cargo check --no-default-features --features
  ws,gen_model,manifold,project_hd` 均通过（只改注释，无行为变更）。
- [ ] T040b（可后置）`getPolygonForFacet` 第二个出参的**负号 = 硬边**
  （`D2_Span::leadsSmoothlyTo` 为假时取负），是曲面法向该怎么分组的权威来源，
  与 `d0088e93 fix(geom): smooth curved-surface normals` 同一件事。本期不实现，
  单独开规格前先把这条记着。
- [ ] T041（依赖 T038+T045）`../vendor/old-aios-core/src/prim_geo/cylinder.rs`、
  `../vendor/old-aios-core/src/prim_geo/sphere.rs`、`src/fast_model/pdms_inst.rs`：
  柱与球的 `hash_unit_mesh_params()` 混入
  段数，`gen_unit_shape()` 相应带段数；`canonical_unit_param_json` 的
  `CYLINDER_GEO_HASH` 特判改按新键走。
  门：不同半径两根柱 `geo_hash` 不同且各自段数正确；同段数两根仍共享一行；
  沿用 2026-08-13 的双键 `param` 回归测试。
  **改身份 = 整库重建**（ADR-044 决策 6），必须与 T045 的爆炸半径结论一起决策。
- [x] T042（2026-08-23 完成）`src/fast_model/libgm_discretise.rs`、
  `src/fast_model/manifold_tessellate.rs`、`src/fast_model/sweep_mesh.rs`：
  **不只是加了一条断言——写断言时发现生产路径上真有第二个容差来源。**
  `sweep_solid_mesh` 一直用 `sweep.tol()`（= 0.01 × 轮廓外接球半径）喂
  `profile_loops` → `arc_segments`，正是 `FACET_TOL_MM` 文档点名不能沿用的那种比例量：
  `tol/R` 恒定 ⇒ 段数与尺寸无关 ⇒ 同一个半径的弧在墙上与在与它相交的原语上分成不同
  段数 ⇒ `cancelFacets` 只消全等重叠 ⇒ 共面处留一层壁。已改为全局绝对量。
  `FACET_TOL_MM` 从 `manifold_tessellate` 迁到 `libgm_discretise`（段数规则与它喂的
  容差住在一起，「唯一一份」才不只是句注释），模块文档里那句「口径尚未对齐」同步作废。
  门：`the_facet_tolerance_has_a_single_source`——四个几何模块的生产半区逐行扫，
  `BrepShapeTrait::tol()` 不得出现在**代码**位置（注释里点名反面是允许的），
  且常量只许定义一处。
  **2026-08-24 收口**：常量只定义一处不等于「唯一一份」。折线化那三条路
  （`manifold_tessellate::profile_spans_of` / `tessellate_arc_wall`、
  `sweep_mesh::flatten_loop`）各留着 `if chord_tol > 0.0 { chord_tol } else { 1.0 }`,
  第二个值藏在分支里、只在非正容差时现身，上面那条源码扫按 `.tol()` 找，扫不到它。
  今天不可达（生产喂的都是 `FACET_TOL_MM`），可容差一旦接成配置项或按构件算，
  0.5mm 就会**静默**变成 1.0mm。三处改为 `libgm_discretise::chord_tol_is_usable`
  判定后 `bail!`。补两道门：`the_chord_tolerance_has_no_fallback_default`
  （源码扫 `let tol` / `let chord_tol` 的右手边不许有浮点字面量——只扫绑定行，
  因为规则函数内部本来就有一堆字面量，`part_rev_segments(r, tol, 0.0, deg)` 的
  `0.0` 是起始角）与两条行为门 `a_non_usable_chord_tolerance_is_rejected_not_defaulted`
  （`0.0` / `-0.5` / `NaN` 一律 `Err`，同一批输入在 `FACET_TOL_MM` 下必须通）。
  回退成兜底写法即红，已实测。全量 `--lib` 1067 通过（余 1 条红是既有的
  `rounded_equal_snout_hashes_produce_one_canonical_param`，修复在 vendor 侧未提交）。
  **未验收的部分要说清楚**：这会改变墙的弧段段数，而能量它的 RVM 门（WP-J）要等
  T043 把 `mesh_compare` 从 `occ` 解绑才跑得起来。目前只有纯函数单测与体积门
  （全量 `--lib` 1061 通过，余 2 条是下面记着的既有红测）。

## WP-H 验收能力解绑（ADR-030 决策 10）

- [x] T043（代码 2026-08-23 落地）`src/rvm_baseline/mesh_compare.rs`：
  `mod gen_side` 的 gate 从 `#[cfg(feature = "occ")]` 改成 `#[cfg(feature = "manifold")]`；
  gen 侧形状由生产同款 `tessellate_libgm_param` 裁决，OCC 降为可选参照分支
  （带 `occ` 才编译），拿不到就落磁盘 `.mesh`。
  门的前一半已验：CI 口径（ws,gen_model,manifold,project_hd，**不带 occ**）
  `--lib` 编译通过（2026-08-24 复核）。**后一半（真跑出对拍结果）未验**——
  它就是 WP-J 的 RVM 门本身，随 T046–T049 一起收。
- [x] T044（2026-08-24 完成）`src/fast_model/loop_model.rs` 那条 `occ` 断言改成对 manifold
  结果断言：`structural_floor_extreme_fillet_remains_finite` 整体断言改走生产同款
  `tessellate_libgm_param`（原 `occ` 块在 CI 口径下根本不编——一条从来不跑的断言），
  AABB 走 `mesh_primitives::compute_aabb`。
  `src/plug_in/water_calculation.rs`：**确认浸水 STP 路径确实是空的**后删除死分支——
  `opencascade_rs` 在任何 Cargo.toml 里都没定义过，发布二进制永远编占位实现；
  `export_stp` 仓内唯一调用点 `test_api/test_water_calculation_stp.rs`（1382 行）
  自己的 `mod` 声明就是注释，连测试都到不了。真 BRep 导出 + 死测试文件一并删除
  （历史在 git），占位实现的文档写明来龙去脉与重启路径（ADR-030 决策 6：独立
  feature 起步，不进 release default）。
  **2026-08-24 追记**：随后另一会话把整个模块（占位 `export_stp` + arango 辅助函数
  + `consts` 集合名）全部删除——业务上不再需要，仓内零调用点，占位实现不复存在；
  重启路径以 changelog 那条「删除浸水插件」为准。

## WP-I 活库盘点（FR-008）

- [x] T045（可并行，无代码依赖；结论是 T041 的前置）一条 SurrealQL 统计 `inst_geo.param`：
  (1) 各 `PdmsGeoParam` 变体实例数与 `geo_hash` 去重数；
  (2) `PrimExtrusion` 中 `cur_type` 为 `Spline` 的行数（预期 0，确认 T036 是纯防御）；
  (3) `PrimRevolution` 中 `rot_dir.z` 非零的行数（预期 0，确认 T033 是纯防御）；
  (4) 柱 / 球的真实半径分布 → 按 T038 的规则算出改键后 `.mesh` 份数（T041 的爆炸半径）。
  结论写回 `specs/009-retire-occ/plan.md`，并决定 T041 是否本期做。
  **2026-08-23 已执行（@8009 只读）**：三个专项全为 0（样条 0/2007、出平面轴 0/42、
  Unknown/Compound 0），T033/T036/T037 确认纯防御；单位柱 99 实例 14 半径折 7 个段数
  等价类，改键后 `.mesh` 1→7 份、球 0 实例——**裁决 T041 本期做**。顺带证得写死 32 段
  对两根大半径柱（r=295 / 324.5mm）弦高 1.42 / 1.56mm、超容差约 3 倍。
  **同日第二个库交叉验证**（`.surreal/ams-7997-e3d-test-20260805` 副本，`inst_geo`
  8,094 行、单位柱 21,354 实例）：三个专项同样全为 0；段数等价类 **37**（改键后
  `.mesh` 1→37）；按实例加权，写死的 32 段**只有 2.0% 是对的**，90.8% 过细。
  排期按大的那个（37）算。证据 `docs/evidence/2026-08-23-occ-retire-census.md`，
  两份已合并进 plan 的 WP-I 一节。
- [ ] T045a（新，依赖 T045）已从 E3D 2.1/3.1 的安装项目源库找到 **3,056 个 SLCY
  （SSCL 源 noun）和 1,448 个 POHE**；E3D 3.1 PlantSample 的 4 个 POHE 已按子记录闭包
  组装并全部通过生产 tessellator（合法索引、非退化 AABB、正体积）。**35,133,637 个索引
  元素仍未发现 SPHE**，因此球体现场门仍保持未完成；不得用纯函数测试替代。

## WP-K 形状摆位对齐（2026-08-24 IDA，T011 顺带查出）

> 参数顺序对不代表几何对。下面两条是**实体位置**错，不是段数错：段数不对是布尔收不敛，
> 摆位不对是构件长在别处。两条都只影响偏心 Snout（`poff ≠ 0`）。
>
> **2026-08-24 T052 已量**：库 A 112 行**全不偏心**，库 B 3 行里**偏心 1 行 2 实例**
> （`poff = 12.06`，错位 6.03 mm ＝ `FACET_TOL_MM` 的十二倍）。T050 本期做，
> 且那一件正好当它的 RVM 验收样本。T051 另有前置（T052a），**先不排期**。

- [ ] T050（串行）`src/fast_model/mesh_primitives.rs`、
  `../vendor/old-aios-core/src/prim_geo/snout.rs`：偏心 Snout 的偏移改成**上下各摊一半**。
  依据：`GM_Snout::calcFacetsWithoutSurfaces`（libgm 3.1 `0x1009EA30`）底圈顶点是
  `(rBtm·cosθ − xShift/2, rBtm·sinθ − yShift/2, −h/2)`、顶圈是
  `(rTop·cosθ + xShift/2, rTop·sinθ + yShift/2, +h/2)`；`calcRange`（`0x1009E900`）的
  支撑函数 `(xShift·dx + yShift·dy + height·dz)/2` 独立佐证同一约定，与 `GM_Pyramid`
  （T016）完全同构。
  本仓现状：`gen_snout` 把偏移整个加在顶圈、底圈不动；aios-core 的 OCC
  （`gen_occ_shape`：`p0` 无偏移、`p1 += poff·b_dir`）也是同一写法。
  **两条后端互相一致，所以此前任何双后端对比都发现不了**——相对 E3D 整体平移
  `(XOFF/2, YOFF/2)`。
  门：(a) 纯函数单测钉住底圈中心 = `(−xShift/2, −yShift/2, −h/2)`、顶圈中心 = 相反数，
  体积不变（卡瓦列里）；(b) **必须有一件真实偏心异径管过 RVM 门**才算验收——只读源码
  就改摆位，等于用一个未验证的结论覆盖另一个未验证的结论。

  **2026-08-24：(a) 已落地。** `gen_snout` 改成 `±offset/2`（侧面环、两个 cap 的中心点
  与 cap 环全都跟着改），新增判别性单测
  `the_eccentric_offset_is_split_between_the_two_ends`：
  用 T052 查出的那件真实构件的尺寸（`poff = 12.06`、`pbdm/ptdm` 66.33/84.42、高 115.2），
  钉住两端环心与「相对位移仍等于整个偏移」，并复核体积不变。
  环心取该端顶点的**包围盒中点**而不是形心——缝合线的 θ=0 顶点与 cap 中心点是刻意
  复制出来的，形心会被带偏 0.34 mm，够把断言变成噪声（第一版就是这么红的）。
  旧的 `eccentric_snout_keeps_volume_and_shifts_top` 包围盒期望同步改了。
  `mesh_primitives` 24 全绿；全量 `--lib` **1068 通过 / 1 失败**，那一条是本文件
  「既有红测」记着的 vendor 未发布问题，与本条无关。`cargo fmt` + `cargo check` 通过。

  **2026-08-24：vendor 侧同批对齐。** `../vendor/old-aios-core/src/prim_geo/snout.rs`
  新增 `LSnout::end_centers() -> (Vec3, Vec3)`，两端圆心的算法收成一处，
  `gen_occ_shape` 与 `gen_brep_shell` 都改调它——**不留两套行为**：OCC 虽然在 T037 之后
  已不是形状引擎（只在 `mesh_compare` 当可选参照），但一个说得清的约定不该有两个版本，
  下一个人不会去分辨哪一份是「故意错着的」。
  抽成方法还有第二个好处：约定本身变得**可测且不需要 `occ` / `truck` feature**——
  `the_eccentric_offset_is_split_between_the_two_ends` 直接钉 `end_centers` 的返回值，
  同样以库 B 那件真实构件为样本，并补一条同心 Snout 必须仍在轴上的反例。
  `cargo test --manifest-path ../vendor/old-aios-core/Cargo.toml --lib
  --no-default-features --features gen_model,sql prim_geo::snout` 2 条全绿
  （vendor 的 `default` 带 `occ`，本机 opencascade-sys 编不过，走 CI 同款无 occ 口径）。
  **仍未发布**：`Toggle-LocalDeps` 是 OFF，本仓编译取 git rev `29c91f48`，所以这条改动
  对 gen-model 的构建**暂时没有效果**。它和 T002 的规范化修复压在同一个文件上，
  一次推上游（见 `docs/plans/2026-08-24-occ-retire-endgame-plan.md` 的 Phase V）。

  **(b) 仍欠**：依赖 WP-J（T046–T049）。T043 的解绑已落地，`mesh_compare` 在不带 `occ`
  的口径下编得过了，但**一次对拍都还没跑过**——这条的验收和 T041 / T038a 一样压在同一
  个闸后面。T052 已经把验收样本找出来了（库 B 那一件），塞进 T049 即可。
  注意两条后端现在**一起改了**，所以 RVM 门对的是 E3D 导出的基准，不是互比。

- [ ] T051（可与 T050 并行）`../vendor/old-aios-core/src/prim_geo/snout.rs`、
  `src/fast_model/manifold_tessellate.rs`：把 Y 方向偏移接通。
  依据：`gm_CreateSnout` 第 4 / 5 参数是 `xShift` / `yShift` 两个方向（`GM_Snout` `this+8`
  / `this+9`，`getXShift` / `getYShift`）；Core3D `CSG_BasicSNO`（`0x10727450`）确实读
  `ATT_XOFF` 与 `ATT_YOFF` 两个属性。
  本仓现状：`LSnout` 只有一个 `poff`，`tessellate_libgm_param` 的 `gen_snout` 调用把
  y 方向**硬写 `0.0`**；`LSnout::From<&AttrMap>` / `From<&NamedAttrMap>` 只读
  `HEIG` / `DTOP` / `DBOT`，XOFF 与 YOFF 一个都没读——目录路径给 `poff` 赋值的那处要一并查。
  ~~**先查再改**：`poff` 承载的是 XOFF 还是沿 `pbax_dir` 的合成偏移，源码没写清楚。~~
  **2026-08-24 T052 已答**：两库全部 115 行的 `pbax_dir` 恒为 `[1,0,0]`、`paax_dir` 恒为
  `[0,0,1]`，`poff` 就是 XOFF。
  **但本条仍不排期**，前置换成 T052a：`inst_geo` 上问不出「有没有 YOFF ≠ 0 的构件」，
  得回 dabacon 侧数。不知道有没有用户，就不知道该加字段还是该在解析处 `bail!`。
  门：属性映射单测（XOFF/YOFF → 两个方向的顶圈位移）；`hash_unit_mesh_params` 已对
  `poff != 0` 走全量哈希，加字段不改身份键——但要有一条测试钉住这一点。

- [x] T052（2026-08-24 完成）统计活库里 `PrimLSnout` 中 `poff` 非零的行数与实例数。
  证据 `docs/evidence/2026-08-24-eccentric-snout-census.md`（两个库，含全部查询）。
  **裁决：T050 不是纯防御，本期做。**
  - 库 A（`ams-7997` 副本 @8039，`inst_geo` 8,094）：`PrimLSnout` 112 行，
    `poff` **全等于 0.0**——都是靠 `ptdm/pbdm` 比值区分的单位锥台，走复用路径。
  - 库 B（`@8009`，`inst_geo` 3,637）：`PrimLSnout` 3 行，**偏心 1 行 2 实例**
    （`poff` 12.06，`pbdm/ptdm` 66.33/84.42，高 115.2；真实尺寸不入复用，
    与 `gen_unit_shape` 对 `poff != 0` 直接 `clone` 一致）。
  - 数量小但错的是**位置**：这一件被整体挪了 `poff/2 = 6.03 mm`，是
    `FACET_TOL_MM = 0.5` 的十二倍。它同时是个现成的验收样本，T049 的曲面原语抽检里
    加一条即可——库 A 反而给不出。
  - **顺带关掉 T051 的一个疑问**：两库全部 115 行的 `paax_dir` 恒为 `[0,0,1]`、
    `pbax_dir` 恒为 `[1,0,0]`，所以 `poff` 沿局部 X，就是 XOFF，不是什么合成偏移。
  - **教训记一条**：T045 那次三个专项在两库都是 0，于是「两库一致」成了默认预期；
    这次两库结论相反。再有「预期为 0」的专项，**一个库查出 0 不能当证明**。

- [ ] T053（新，2026-08-24 从 T038a 的验收缺口里撞出来；**T041 / G3 的范围要重写**）
  **参与单位网格复用的曲面原语不止柱与球，段数规则现在全都喂到的是单位半径。**
  库 A 实测：`PrimCTorus` 95 行 `rout` 全等于 1.0、`PrimRTorus` 167 行同样、
  `PrimLSnout` 112 行 `pbdm` 全等于 1.0、`PrimDish` 17 行 `pdia` 全等于 1.0。
  于是 `elliptical_dish_facets(0.5, h, 0.5)` 里 `tol/R = 1.0`，角步长直接撞 45° 封顶,
  **任何尺寸的碟都得到 8 段**。碟的 102 个实例落在 21 个不同 scale 上，13 mm 到
  48,900 mm；最大那件应当是 492 段，弦高 1,861 mm ＝ 容差的 3,700 倍。
  影响三处：
  (1) **T041 的爆炸半径估错了**——plan 的 G3 只按单位柱的 37 个等价类算，实际至少还要
      加碟 / 两种环面 / 同心 Snout 四类，各自数等价类，整库重建的代价要重估；
  (2) **T038 / T038a 的「已完成」要加限定语**：规则正确、单测全绿，但生产路径上还没有
      一个复用型曲面原语真按真实半径分段。真值表里那些 ✅ 说的是「规则对」，不是
      「现场生效」；
  (3) 顺带一个**未验证**的可疑点：`Dish::hash_unit_mesh_params` 哈希的是未归一化的
      `prad`，而 `gen_unit_shape` 落库的是 `prad/dia`——与 snout 那条 T002 已修的
      双键问题同一形状。只是读码所见，没构造用例，别当结论用。
  证据：`docs/evidence/2026-08-24-unit-normalised-curved-primitives.md`。
  **先做的是把范围写清楚，不是急着改键**：改身份 = 整库重建（ADR-044 决策 6），
  五类一起改和只改柱是两个量级的决策。

- [x] T052a（新，从 T052 拆出）**已直接在 dabacon 源属性完成普查。**
  `LSnout` 结构里只有一个 `poff`，源数据的 YOFF 是多少都会在落库时消失——查出 0
  不构成证据（这正是静默失效：判据依赖的基准数据本身已被判据要查的那个缺陷抹掉了）。
  要回答得回 **dabacon 侧**数 SNOU 元素的 `YOFF` 属性。
  E3D 3.1 安装项目找到 214 个非零 YOFF SNOU/NSNO；2.1+3.1 合计 422 个。代表样本
  `pe:15207_10558` 为 `DBOT=2000, DTOP=700, HEIG=1200, XOFF=0, YOFF=650`，输入文件
  SHA-256 为 `81bbacbbb5d272b3c6e90342e240ebc8ef93f98e3b8098f444049a3eb7101ca7`。
  该样本经生产 tessellator 得到 404 顶点、400 三角形、正体积；AABB Y
  `[-1325, 675]` 同时钉住 `±YOFF/2` 摆位。证据见
  `docs/evidence/2026-08-24-occ-retire-source-census.md`。

## 既有红测（不是本规格引入的，记在这里免得每次重新排查）

2026-08-23 全量 `cargo test --lib --no-default-features --features
ws,gen_model,manifold,project_hd`：T039 / T042 落地后是 **1062 通过 / 1 失败**。
两条都不是本规格引入的，其中一条已顺手修掉：

- `fast_model::pdms_inst::tests::rounded_equal_snout_hashes_produce_one_canonical_param`
  ——**结构性的**。修复在 `../vendor/old-aios-core/src/prim_geo/snout.rs`
  （`gen_unit_shape` 把 `ptdm/pbdm` 按 `f32_round_3` 收敛到与哈希身份同一个值），
  但那份改动只在本地 vendor、**没合上游**，而 `Toggle-LocalDeps` 当前是 **OFF**，
  所以编译取的是 git rev `29c91f48`，不含该修复。这正是 T002 说的「合上游前相关项
  视为未完成发布」。开 `-On` 即绿。
  **2026-08-23 复核**：`git -C ../vendor/old-aios-core diff src/prim_geo/snout.rs`
  确认那一行是 `let ptdm = f32_round_3(self.ptdm / self.pbdm);`——`gen_unit_shape`
  落库的值与哈希身份取同一个规范值。解阻只有一条路：把 vendor 那份提交并推上去，
  再升 `Cargo.toml` 的 aios-core rev（T002 的收尾）。在那之前这条红是**预期的**，
  它正确地报告着「有个修复还没发布」。
- ~~`data_interface::cata_closure::tests::locator_scan_failure_is_a_result_and_cannot_cache_an_empty_success`~~
  ——**2026-08-23 已修**。一条失效的守护：它按源码字面找
  `scan_db_ref0s(&entry.path, project)?`，而依赖身份清单那一版把
  `build_for_project` 里的这一处换成了
  `scan_identity_ref0s(&entry.path, &entry.db_type, &entry.project)?`，针没跟着改，
  `find(...).unwrap()` 于是在 `None` 上炸开。
  **上一版记录的诊断是错的**：调用点并没有「移出 `build_for_project`、改成
  `&sel.leaf_path`」——`src/data_interface/cata_closure.rs:608/613` 那两处属于另一个
  函数，一直是 `scan_db_ref0s`。`build_for_project` 里的扫描 + `cache.put` 一直都在
  （`:392/393`），**它要钉的不变量从头到尾成立，只是没人在钉**。
  修法是把针改到新名字，并把两处 `unwrap()` 换成带话的 `expect()`——下一次改名至少
  读得到「谁没跟上」，而不是一个空 `None`。

## WP-J RVM 门（依赖 T043 + WP-G 落地）

- [x] T046（2026-08-24 完成）直墙 RVM（T019 的门）。
- [x] T047（2026-08-24 完成）弧墙 + 360° SANN 体积门 RVM（T020 的门，FR-006）。
- [x] T048（2026-08-24 完成）斜切墙 RVM（T022 / T023 的门）。
- [ ] T049 曲面原语抽检 RVM：柱 / 球 / PrimLSnout / 碟 / 圆环面各一，段数改动后重建基准。
  阈值一律不放宽（FR-010）；证据进 `docs/evidence/`，`docs/2026-08-12_live-test-ledger.md`
  同步。

  **2026-08-24 双副本执行结果**：RVM 测试现可用 `AIOS_RVM_DB_ENDPOINT` 和
  `AIOS_RVM_MESH_DIR` 指向隔离副本。默认 8009 的四堵 WALL gen→RVM p95
  `7.86/7.84/8.63/4.05mm`，四堵 STWALL 双向 p95 全为 0；7997 副本从生成根重建后，
  20/20 GWALL union 的 gen→RVM p95 为 4.14mm，三件大体量墙为
  `0.1/9.3/167.5mm`，均过既有门。该副本没有 8 条历史测试专用 WALL/STWALL
  `inst_relate`，因此新增从各副本自己的 PE/CATA/SPINE 源属性走生产解析/三角化的门；
  两副本均得到 WALL p95 `7.86/7.83/8.62/4.05mm`、STWALL 双向 p95 全 0。
  360° SANN 与两个半环体积误差 ≤1% 的纯函数门同批通过。证据：
  `docs/evidence/2026-08-24-occ-retire-dual-copy-rvm.md`。


## 2026-08-24 独立工作树执行记录

- [x] T041-code / T053-code：六类复用曲面接入 `FacetCaliber`；hash 与 `gen_unit_shape()` 同键；
  主仓删除 `unit_mesh_identity`，缺 caliber 返回原子整库重建错误；普通 LCylinder/SCylinder
  同 caliber 仍规范成单一 `PrimLCylinder` 参数。**双库容量、重建与 RVM 仍未完成。**
- [x] T050-code：Snout `±offset/2` 与规范化修复已发布；**现场 RVM 未完成。**
- [x] T051-code：XOFF/YOFF 两轴已贯通解析、hash、单位参数和 Manifold；旧 XOFF 仍映射 `poff`。
- [x] T023-code：斜切延伸已进入 Manifold CSG；`sweep_mesh` 32 条单测全绿，T048 仍待现场 RVM。
- [x] T026/T027/T028：源码护栏钉住退役 Core3D 操作、目录负体 Manifold 唯一入口，
  并钉住有效 `PlantMesh` → 持久化 → AABB 角点回执顺序；3 条定向测试全绿。
  T052a 的 dabacon YOFF 现场普查已完成：E3D 3.1 非零样本 214 个，代表件已过生产网格验证。
- [x] T002-release：无 OCC 的 aios-core `f9551ef4`、parse `ac85df94`、pdms-io `c4f02e97`
  已发布并同步升级根包与 Python；local-deps OFF 的依赖图只有一份 aios-core。
- [x] T031-code：主仓、Python、CI 与发布流程已删除 OCC feature、依赖、API、布尔和 DLL
  装配；无 Manifold 后端时 `gen_inst_meshes` 明确失败；新增源码/manifest/workflow 删除护栏，
  并修正 wheel OCCT 清单检查读取了错误目录的问题。**最终现场发布仍按硬门暂停：SSCL、
  多面体、YOFF 已有真实源样本和生产网格证据，球体 SPHE 与双库 RVM 仍缺。**
- [x] T054-code：`ProfileRing` 显式保存逐边光顺关系；libgm `1e-6` 切向判据、反向重排、
  端盖硬边、Manifold 属性顶点和布尔后的法线传播均有纯函数/网格回归测试。
- [x] T032-code：`cargo fmt --check`、默认与无默认 feature `cargo check`、全量 lib、四个
  CI 集成目标和 Python offline 测试通过；维护窗口、双库 RVM 与 live ledger 留待现场硬门。
