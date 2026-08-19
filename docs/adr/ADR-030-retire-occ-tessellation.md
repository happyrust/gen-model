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
