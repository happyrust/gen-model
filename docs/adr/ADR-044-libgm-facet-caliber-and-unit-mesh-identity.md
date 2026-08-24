# ADR-044：曲面段数按 libgm 真实半径算，单位网格身份随之带上段数

状态：Proposed（2026-08-23）

关联：ADR-026（扫掠体步骤与**单位网格身份**——本 ADR 改的就是那把身份键）；
ADR-030（分阶段退役 OCC 三角化，含 2026-08-23「IDA 修订二」）；ADR-029（布尔走
manifold-csg）；ADR-002（几何权威在 core3d / 已解析参数）。
逆向全文：`plant-4/libgm-boolean-algorithm.md` §7.9 / §7.9.1 / §6.11。
术语见 `CONTEXT.md`「单位几何 / 单位网格身份 / 实例变换」。

## 背景

`src/fast_model/libgm_discretise.rs` 已经把 libgeom 的 `d2_numberOfSegmentsForCircle`
与 `d2_numberOfSegmentsForPartRev` 照抄进来了，挤出 / 回转 / 扫掠三条截面路径也已经改用
它。**但曲面原语没有。** `tessellate_libgm_param` 里：

| 原语 | 现状 | libgm §7.9.1 的口径 |
|---|---|---|
| `PrimLCylinder` / 无切角 `PrimSCylinder` | `tessellate_unit_cylinder(32)` | `circle(radius, tol)` |
| 切角 `PrimSCylinder`（SSCL） | `gen_slope_ended_cylinder(…, 32)` | `circle(radius, tol)` |
| `PrimSphere` | `unit_sphere()` = `gen_sphere(0.5, 16, 36)` | `circle(radius, tol)`；经向恒 n/2（见 2026-08-24 修订） |
| `PrimLSnout` | `gen_snout(…, 32)` | `circle(fmax(rBtm, rTop), tol)` |
| `PrimDish` | `gen_*_dish(…, 32)` | 球碟 / 椭圆碟各一套，两段分算 |
| `PrimCTorus` | `gen_circular_torus(…, 32, 16)` | 扫掠 `partRev(rOut, …)` 非整圈 **+1**；管截面 `circle((rOut−rIns)/2, …)` |
| `PrimRTorus` | `gen_rectangular_torus(…, 32)` | `partRev(rOut, tol, s, e)` |

`libgm_discretise` 自己的单测写明：`circle_segments(100.0, 0.5) == 32`——**32 只在
R=100 配 0.5mm 容差这一个尺寸上对**。活库里的柱子从 DN15 到几米都有。

这不是画质问题。`GM_Facets::doFacetCancellation` 的 `cancelFacets`（§6.11）**只消全等
重叠**：共面的两层侧壁段数差一段，共面抵消就整个放弃，布尔结果里留一层内壁。段数规则
因此是布尔能否收敛的前置条件。

挡在前面的是身份：`LCylinder::hash_unit_mesh_params()` 返回常量 `CYLINDER_GEO_HASH`，
`gen_unit_shape()` 返回 `Self::default()`——全库所有圆柱共享一行 `inst_geo` 与一个
`.mesh`，真实半径只活在实例变换里。一份网格没法同时是 16 段和 484 段。

`GM_Profile::setNSteps`（libgm 3.1 `0x1008F2E0`）还揭示了第二条口径：回转与 collar 的
轮廓离散**不是**每 span 自算，而是「自身半径与配对 span 半径取大」再与已存步数取大。
`GM_Extrusion::calcFacets`（`0x10056F10`）不走这条路。本仓目前只有挤出那一套。

## 决策

1. **曲面原语的段数一律由真实半径与弦高容差算出，规则只有 `libgm_discretise` 一份。**
   每个原语喂哪个半径照 §7.9.1 的调用点表逐条钉；不得凭「差不多」挑数，也不得在
   `mesh_primitives` 里再写第二个默认段数。`DEFAULT_CIRCULAR_SEGMENTS` 与散落的字面量
   `32` / `16` 从生产路径上清掉。

2. **单位网格身份带上段数。** `hash_unit_mesh_params()` 对曲面原语混入该实例算出的段数
   （柱、球两类受影响；PrimLSnout / 碟 / 环面本来就烤真实尺寸）。段数是 4 的倍数且落在
   `[8, 1000]`，圆柱最多裂成 249 份网格而不是 1 份——有界，且远小于按真实半径逐个建。
   `SCylinder` 的 SSCL 分支早就是「整参数哈希」，本决策与它同向，不是新范式。

3. **回转轮廓走 `setNSteps` 那一套，挤出保持 `getApproxPolyLine`。** 两条口径在
   `libgm_discretise` 里各出一个入口，禁止合并成一个「通用轮廓离散」。合并等于在
   REVO/NREV 上继续用挤出的段数，而那正是当前的缺陷。

4. **容差是全局量，不是每原语自算。** libgm 的 `arctol_` 在创建原语时读一次烤进对象，
   Core3D 主初始化传 0.5mm。本仓统一用 `FACET_TOL_MM`，不得回到 `PdmsGeoParam::tol()`
   那种「按自身包围球半径给相对容差」的做法——相对容差会让相邻两个尺寸不同的构件在
   共面处拿到不同段数，共面抵消随之失效。要做成 `DbOption` 可配时，仍只能有一个来源。

5. **改身份就要改共享行的收口。** `pdms_inst.rs::canonical_unit_param_json` 现在按
   `geo_hash == CYLINDER_GEO_HASH` 特判统一 `param` 变体；身份键变了，这条特判要跟着
   按新键走，不得留下「两个变体并进一个对象」的老坑（2026-08-13 那次 2,229 根全灭）。

6. **迁移是一次整库重建，不是就地改写。** 身份键变化意味着旧 `geo_hash` 全部作废；
   按首次导入重解析（与 ADR-021 的文件回退同一条路），不得让新旧两套 `geo_hash` 在库里
   共存——共存就没人说得清某一行的段数是按哪套算的。

### 2026-08-24 修订（`GM_Sphere::calcFacetsWithoutSurfaces` `0x100A20F0` 反编译）

球的两个方向此前只钉了绕轴那一半，经向是猜的。现在两半都有权威：

- 绕轴 `n = circle(自身半径@this+5, tol@this+3)`；`n > 1000` 打
  `GM_Sphere - facet tolerance too small for radius, adjusted` 后**硬截 1000**
  （曲面原语那条路，非轮廓路的整体重标定）。
- 经向带数**恒 = `n/2`**：`GM_Facets` 构造实参 `(n²/2, n(n−1), n·(n/2−1)+2)`，
  两极各一个顶点、中间 `n/2 − 1` 圈——经向沿用绕轴同一个角步长，与球碟同构。

对决策 2 的收紧：**球的身份键只需混入一个 n**，stacks 不是独立自由度。顺带把
「现状 16×36」钉死为错：R=100 / tol=0.5 的「幸运尺寸」下 E3D 是 32 slices ×
16 stacks——stacks 恰好对，**36 这个 slices 从来没对过**。两个盘点库都没有球实例
（T045a），改键前唯一的保证就是这条规则本身，所以它必须来自反编译而不是推断。
证据 `docs/evidence/2026-08-24-ida-occ-retire-audit.md`。

## 后果

- 圆柱 / 球的 `.mesh` 份数从 1 涨到「实际出现过的段数种类」数量级；`EXIST_MESH_GEO_HASHES`
  与磁盘占用相应上升，需要在活库盘点（ADR-030 FR-008）里量出来再定。
- 段数改变会让**所有**曲面构件的网格哈希变化，RVM 对拍必须重新建基准；这正是
  ADR-030 决策 10 要求先把 `mesh_compare` 从 `occ` 解绑的原因。
- 共面抵消开始按 E3D 的口径收敛，布尔后残留内壁一类缺陷应当消失；若不消失，说明段数
  之外还有相位问题（§7.9.0 的角度格子），那是另一条线索而不是放宽阈值的理由。

## 否决方案

- **把段数固定成一个更大的数（如 128）。** 段数要的是与 E3D **相等**，不是足够多；
  不等就不抵消。
- **保留单一单位圆柱，靠实例变换缩放。** 缩放改不了段数，这是拓扑量。
- **给每个真实半径建一份网格（放弃单位几何）。** 段数只有 249 种取值，按半径建会把
  复用度打到接近零，且与 ADR-026 的单位网格身份正面冲突而无收益。
- **回转沿用挤出的每 span 自算，等 RVM 门发现问题再说。** 已经知道两条口径不同，
  留着就是明知故犯的静默失效。
- **用相对容差（`PdmsGeoParam::tol()`）省掉全局容差。** 相邻构件尺寸不同就拿到不同
  段数，共面处永远对不齐。
