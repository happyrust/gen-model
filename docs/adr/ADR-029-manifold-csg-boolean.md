# ADR-029：设计布尔改走本地 manifold-csg，OCC 只保留三角化

状态：Accepted（2026-08-15）

关联：ADR-002（几何权威在 core3d / 已解析参数，不在 OCC 布尔）；生产布尔入口
`apply_insts_boolean_manifold` / `apply_cata_neg_boolean_manifold`。

## 背景

生成管线实际是两套内核：

1. **三角化**：OCC `gen_occ_shape` → `.mesh`（SweepSolid / 挤出 / 原语）。
2. **布尔**：旧 `manifold-sys`（aios-core `ManifoldRust`）在网格上做差集。

OCC 布尔（`apply_insts_boolean_occ`）生产路径已注释。2026-08-15 对
`pe:17496_116569` 实测：一次 `BRepAlgoAPI_Cut` 写出 60 字节空网格，覆盖了
manifold 的 3588 字节结果。

本地已有 `D:\work\plant-code\old\manifold-csg`（manifold3d 安全绑定，f64
`MeshGL64`、`batch_difference`、2D 挤出）。旧 `manifold-sys` 是另一份 FFI，
量化截断、手动 alloc，且与 OCC 布尔并存造成「到底哪套在切洞」不清。

## 决策

1. **第 1 刀（本期）**：设计/目录负体布尔只走 path 依赖
   `../manifold-csg/crates/manifold-csg`。gen-model 的
   `manifold_bool.rs` 不再调用 `ManifoldRust`。
2. **OCC 三角化暂时保留。** SweepSolid（WALL / GENSEC 沿 SPINE 扫掠）在
   manifold-csg 里没有等价物；没有新的脊线网格器之前，不得从 default /
   release 拿掉 `occ`。
3. **OCC 布尔不进生产。** 空结果不得覆盖已有 `booled_id` 网格。
4. **不在本期改 aios-core 的 `manifold-sys` 依赖。** 布尔入口只在本仓换实现；
   aios-core 仍可编译旧 FFI，gen-model 不再使用。第 2 刀再把挤出类三角化
   迁到 `CrossSection::extrude`，并在 aios-core 删 `manifold-sys`。

## 后果

- 布尔与三角化职责分离：切洞失败不再被理解成「OCC 没开」。
- 首次编译会构建 manifold3d C++（或使用 `MANIFOLD_CSG_LIB_DIR` 预编译库）。
- `.mesh` 由索引网格写出（不再按三角打散顶点）；对拍读 `PlantMesh` 顶点即可。
- SweepSolid / snout / torus 等仍依赖 OCC，直到第 2 / 第 3 刀。

## 否决方案

- 一次从 default/release 删除 `occ`：没有 SweepSolid 网格器，生成会停。
- 继续用 OCC 布尔补 manifold 薄片：已被 116569 空网格证伪。
- 先改 aios-core 再改本仓：本仓布尔是唯一生产调用点，先换调用点即可验证。

## 2026-08-19 Oracle 审核修订

布尔失败采用显式 `GeometryFailurePolicy`：活动暂存窗口为 `Required` 并阻断水位；窗口外按需、延迟及基线后补为 `BestEffortFallback`，保留诊断和旧几何而不形成永久死信。
