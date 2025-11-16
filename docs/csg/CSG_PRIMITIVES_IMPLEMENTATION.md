# CSG 基本体实现方案

## 概述

本文档说明如何在 `rs-core` 项目中实现三个基本体的 CSG mesh 生成：
1. **PrimCTorus** (圆环体/Circular Torus)
2. **PrimPyramid** (棱锥体/Pyramid)  
3. **PrimRTorus** (矩形环面体/Rectangular Torus)

## 实现位置

实现需要在 `rs-core/src/geometry/csg.rs` 文件中的 `generate_csg_mesh` 函数中添加对应的 match 分支。

## 基本体参数结构

根据代码分析，基本体参数定义在 `aios_core::parsed_data::geo_params_data::PdmsGeoParam` 中：

```rust
// 假设的结构（需要根据实际定义调整）
PdmsGeoParam::PrimCTorus(CTorus { rins, rout, angle })
PdmsGeoParam::PrimPyramid(Pyramid { x_bottom, y_bottom, x_top, y_top, x_offset, y_offset, height })
PdmsGeoParam::PrimRTorus(RTorus { inner_radius, outer_radius, height, angle })
```

## 参考实现

参考 `/Volumes/DPC/work/plant-code/rvmparser/src/TriangulationFactory.cpp` 中的实现：
- `circularTorus()` - 第 637-769 行
- `pyramid()` - 第 305-415 行  
- `rectangularTorus()` - 第 502-634 行

## 实现步骤

1. 在 `rs-core/src/geometry/csg.rs` 中找到 `generate_csg_mesh` 函数
2. 在 match 语句中添加三个新的分支
3. 实现每个基本体的顶点和索引生成逻辑
4. 返回 `Some(GeneratedMesh { ... })`

## 注意事项

- 需要根据 LOD 设置计算合适的细分段数
- 法线计算要正确
- 确保顶点顺序符合右手定则
- 处理边界情况（如角度为 0、半径为 0 等）


