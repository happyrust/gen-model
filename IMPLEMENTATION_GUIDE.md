# CSG 基本体实现指南

## 概述

本指南说明如何在 `rs-core` 项目中实现三个基本体的 CSG mesh 生成功能。

## 实现位置

需要在 `rs-core/src/geometry/csg.rs` 文件中的 `generate_csg_mesh` 函数添加实现。

## 步骤 1: 打开 rs-core 项目

```bash
cd /Volumes/DPC/work/plant-code/rs-core
```

## 步骤 2: 找到 generate_csg_mesh 函数

在 `src/geometry/csg.rs` 中找到 `generate_csg_mesh` 函数，它应该类似这样：

```rust
pub fn generate_csg_mesh(
    param: &PdmsGeoParam,
    csg_settings: &LodMeshSettings,
    non_scalable: bool,
) -> Option<GeneratedMesh> {
    match param {
        PdmsGeoParam::PrimBox(box_param) => {
            // 已有实现
        }
        PdmsGeoParam::PrimSCylinder(cyl_param) => {
            // 已有实现
        }
        // ... 其他已有基本体 ...
        
        // 需要添加以下三个分支：
        PdmsGeoParam::PrimCTorus(_) => None,  // 当前返回 None
        PdmsGeoParam::PrimPyramid(_) => None, // 当前返回 None
        PdmsGeoParam::PrimRTorus(_) => None,  // 当前返回 None
        
        _ => None,
    }
}
```

## 步骤 3: 添加辅助函数

在 `csg.rs` 文件中添加辅助函数（参考 `csg_primitives_impl.rs` 文件）。

## 步骤 4: 在 match 中添加实现

将以下代码添加到 `generate_csg_mesh` 函数的 match 语句中：

```rust
PdmsGeoParam::PrimCTorus(ct) => {
    generate_circular_torus_mesh(
        ct.rins,
        ct.rout, 
        ct.angle,
        csg_settings,
        non_scalable
    )
}

PdmsGeoParam::PrimPyramid(py) => {
    generate_pyramid_mesh(
        py.x_bottom,
        py.y_bottom,
        py.x_top,
        py.y_top,
        py.x_offset,
        py.y_offset,
        py.height
    )
}

PdmsGeoParam::PrimRTorus(rt) => {
    generate_rectangular_torus_mesh(
        rt.inner_radius,
        rt.outer_radius,
        rt.height,
        rt.angle,
        csg_settings,
        non_scalable
    )
}
```

## 步骤 5: 检查类型定义

确保以下类型在 `rs-core` 中已定义：
- `PdmsGeoParam::PrimCTorus(CTorus { rins, rout, angle })`
- `PdmsGeoParam::PrimPyramid(Pyramid { x_bottom, y_bottom, x_top, y_top, x_offset, y_offset, height })`
- `PdmsGeoParam::PrimRTorus(RTorus { inner_radius, outer_radius, height, angle })`

如果字段名不同，需要相应调整。

## 步骤 6: 测试

运行测试命令：

```bash
cd /Volumes/DPC/work/plant-code/gen-model
cargo run --bin aios-database -- --debug-model 21491/18957 --capture output/capture-L1 --capture-include-descendants
```

检查是否不再出现警告：
```
[WARN] CSG mesh generation not supported for type: PrimCTorus
```

## 参考实现

- C++ 参考：`/Volumes/DPC/work/plant-code/rvmparser/src/TriangulationFactory.cpp`
  - `circularTorus()` - 第 637-769 行
  - `pyramid()` - 第 305-415 行
  - `rectangularTorus()` - 第 502-634 行

- Rust 实现：`csg_primitives_impl.rs`（本项目中）

## 注意事项

1. **角度单位**：确保角度使用弧度制（rvmparser 中使用弧度）
2. **法线方向**：确保法线指向外部（符合右手定则）
3. **顶点顺序**：确保三角形顶点顺序正确（逆时针为正面）
4. **边界情况**：处理角度为 0、半径为 0 等特殊情况
5. **LOD 支持**：根据 `csg_settings` 调整细分程度

## 调试技巧

1. 先实现最简单的 Pyramid，验证流程
2. 再实现 RTorus，最后实现 CTorus（最复杂）
3. 使用 `--debug-model` 参数查看详细日志
4. 检查生成的 OBJ 文件是否正确


