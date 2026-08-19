# Tasks：按 libgm 原语用 manifold-csg 替换 OCC

**Input**：`specs/009-retire-occ/plan.md`
**Prerequisites**：ADR-030（含 IDA 修订）、ADR-026、ADR-029；宪法 I–III（响亮失败、单一规则）

> 勾选表示本工作树已落地。aios-core 与 `manifold-sys` 解耦目前只在本地 vendor +
> `Toggle-LocalDeps -On`，**不能推 main**，合上游前相关项视为未完成发布。

## 流程与底座

- [x] T001（串行）`docs/adr/ADR-030-retire-occ-tessellation.md`、`specs/009-retire-occ/spec.md`、
  `specs/009-retire-occ/plan.md`：ADR → spec → plan；权威定为 libgm `gm_Create*` 而非 OCC。
- [x] T002（串行）`Cargo.toml`、`../vendor/old-aios-core/Cargo.toml`、
  `../vendor/old-aios-core/src/csg/mod.rs`、`prim_geo/extrusion.rs`、`prim_geo/revolution.rs`：
  `gen_model` 不再捆绑 `manifold-sys`；本仓 `manifold` 只拉 `manifold-csg`。发布前升 aios-core rev。
- [x] T003（串行）`src/fast_model/manifold_csg.rs`、`src/fast_model/manifold_bool.rs`：
  布尔 ingest f64、空网格 hard fail、禁止 `ManifoldRust`（ADR-029 骨架）。
- [ ] T004（串行，依赖 T003）编译并跑 `cube_minus_inner_cube` / `ingest_rejects_empty_mesh`；
  live `mesh_gwall_extra_against_cwall`；更新 `docs/2026-08-12_live-test-ledger.md`。
- [x] T005（串行，依赖 T003）`src/fast_model/occ_generate.rs`：`not(occ)` 且无
  `tessellate_libgm_param` 后端时 `bail!`，禁止 `Ok(())` 静默跳过。

## WP-A / B1–B2 箱柱挤出骨架

- [x] T006（可与 T003 并行）`src/fast_model/manifold_tessellate.rs`、`src/fast_model/mod.rs`：
  单位箱、单位柱、`PrimExtrusion` 轮廓挤出；空挤出 hard fail。五条单测已绿。
- [x] T007（串行，依赖 T005+T006）`src/fast_model/occ_generate.rs`：`gen_inst_meshes` 对
  `PrimBox` / `PrimLCylinder` / 无切角 `PrimSCylinder` / `PrimExtrusion` 先
  `tessellate_libgm_param`，失败或 `None` 再 OCC；AABB/`pts` 改从网格取，禁止空 `pts` 静默。
- [x] T007a（2026-08-19 补口径）`manifold_tessellate.rs`：挤出 FRADIUS 倒角接
  `gen_polyline_original` 权威离散（弦高容差 `Extrusion::tol()`，体积对拍 1% 钉住）；
  样条轮廓（`CurveType::Spline`）回退 OCC，不得折线近似。
- [ ] T008（依赖 T007）抽检 live：一箱、一柱、一挤出网格非空；台账一行。

## WP-B 其余目录原语（可并行，均依赖 T006）

- [ ] T009（可并行）`manifold_tessellate.rs`：`gm_CreateSphere` → 单位球半径 0.5（`PrimSphere`）。
- [ ] T010（可并行）`manifold_tessellate.rs`：`PrimPolyhedron` → `from_mesh_f64`；非流形 hard fail。
- [ ] T011（串行）IDA `idalib-32268`：钉 `gm_CreateSnout` 五参数顺序（xref `0x1073c12c`），
  写入 `specs/009-retire-occ/plan.md` B4。
- [ ] T012（依赖 T011）`manifold_tessellate.rs`：`PrimLSnout` → 锥台 ± 偏心。
- [ ] T013（串行）IDA：钉 `gm_CreateCircularTorus` / `RectangularTorus` 参数；写入 plan B5/B6。
- [ ] T014（依赖 T013）`manifold_tessellate.rs`：圆环面 / 矩形环面 → `CrossSection` + `revolve`。
- [ ] T015（依赖 T014）`manifold_tessellate.rs`：`PrimDish` → 球碟 / 椭圆碟旋转体。
- [ ] T016（依赖 T011，可能依赖 T023）IDA 钉 `gm_CreatePyramid` /
  `gm_CreateSlopeEndedCylinder`；实现 `PrimPyramid` / `PrimLPyramid` / 切角柱。
- [ ] T017（可并行）源码断言：`gm_CreateNull` / Mark / Straight / Arc / Bezier 不得作为
  `tessellate_libgm_param` 的成功分支。

## WP-C 扫掠体（依赖 T006；C0 先于 C1–C4）

> 截面与成体不走 manifold-csg：自建网格，manifold 只用于布尔（与 WP-B 同一决定）。
> 内核落在 `src/fast_model/sweep_mesh.rs`，纯函数单测已绿；**接进 `tessellate_libgm_param`
> 属于 T019/T020/T022 的后半段，要等各自的 RVM 门**，未过门前扫掠体继续回退 OCC。

- [x] T018（串行）截面：SANN / SPRO / SREC → 2D 闭合环（外环逆时针 + 孔顺时针，弧折线）。
  文件：`src/fast_model/sweep_mesh.rs`。倒角与弧段复用 aios-core `wire::gen_polyline_original`，
  折线化用 `arcs_to_approx_lines`，端盖三角剖分用 earcutr（凹截面必须）。
- [ ] T019（依赖 T018）目录截面版 `gm_CreateExtrusion`；直墙 RVM 门。
  内核 `extrude_loops` / `loft_loops` 已绿（矩形、L 形凹截面、带孔圆环体积对拍）；缺生产接线与 RVM。
- [ ] T020（依赖 T018）`gm_CreateRevolution(..., 180°)` 两半合并；弧墙 + 360° SANN 体积门（FR-006）。
  内核 `revolve_loops` 已绿（帕普斯定理 + 与 `gen_rectangular_torus` 交叉对拍）；
  360° SANN 走「外环 + 内孔」一次成形，已与两半之和对拍；缺弧墙 RVM。
- [ ] T021（串行）IDA 反编译 `0x107318E0` 斜切延伸段（Start/End-mitre extension），写成纯函数。
- [ ] T022（依赖 T018；**不依赖 T021**）`gm_CreateRuledSolid`：两端轮廓一一对应连三角。
  斜切端面变换是现成的 `SweepSolid::get_face_mat4`（OCC 的 `Solid::loft` 用的就是它），
  T021 只影响 C4 的额外延伸实体。内核 `loft_loops` 已绿（斜切不改体积）；缺斜切墙 RVM。
- [ ] T023（依赖 T022）斜切延伸挤出挂到段 CSG；斜切墙 RVM；垂直/平行切向不得误走 Ruled（ADR-026）。
- [ ] T024（可后置）多段 SPINE：`transform` + `batch_union`；无多段夹具则只单测。

## WP-D CSG 树

- [ ] T025（可与 T007 并行，依赖 T003）布尔 live：`mesh_gwall_extra_against_cwall` p95≤180；
  空差集不覆盖 `booled_id`（**静态半 2026-08-19 已落地**：设计/目录两条 manifold
  生产路径均在写盘前拦空差集 → `bad_bool` 出声，
  `empty_difference_is_bad_bool_not_a_silent_swallow` 钉住；live p95 未跑）。
- [ ] T026（可并行）源码断言生产路径不调用 Clip/Expand/Compress/SolidTree/Picture/Clash。
- [ ] T027（依赖 T025）确认 `apply_cata_neg_boolean_manifold` 仍是目录负体唯一入口。

## WP-E 离散口径与收口

- [ ] T028（依赖 T007）`occ_generate.rs`：manifold 路径 AABB 来自 `PlantMesh`；`pts` 有明确来源或省略策略写进回执，禁止空列表假装成功。
- [ ] T029（串行，依赖 T008+T023+T025）活库 `PdmsGeoParam` 出现次数盘点（FR-008）；未覆盖类型列表进 plan。
- [ ] T030（依赖 T029）`loop_model.rs`、`rvm_baseline/mesh_compare.rs`、`plug_in/water_calculation.rs`：
  测试与浸水改为网格或独立 feature；不得挡主路径删 `occ`。
- [ ] T031（依赖 T029+T030）`Cargo.toml` default/release 去掉 `occ`；CI 增加无 occ 的
  `gen_inst_meshes` 失败闭合测。
- [ ] T032（串行）`changelog.md`、live 台账、`cargo fmt` / `cargo check`；aios-core 解耦合上游并
  `Toggle-LocalDeps -Off` 后再推。
