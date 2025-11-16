# CSG 拉伸体和旋转体实现方案

## 概述

本文档说明如何在 `rs-core` 项目中实现两个基本体的 CSG mesh 生成：
1. **PrimExtrusion** (拉伸体/Extrusion)
2. **PrimRevolution** (旋转体/Revolution)

## 实现位置

实现需要在 `rs-core/src/geometry/csg.rs` 文件中的 `generate_csg_mesh` 函数中添加对应的 match 分支。

## 基本体参数结构

根据代码分析，基本体参数定义在 `aios_core::parsed_data::geo_params_data::PdmsGeoParam` 中：

```rust
PdmsGeoParam::PrimExtrusion(Extrusion { verts, height })
PdmsGeoParam::PrimRevolution(Revolution { verts, angle })
```

其中：
- `Extrusion::verts` - 2D轮廓顶点（Vec3数组，z坐标通常为0）
- `Extrusion::height` - 拉伸高度
- `Revolution::verts` - 2D轮廓顶点（Vec3数组，x/y坐标表示到轴的距离，z坐标表示高度）
- `Revolution::angle` - 旋转角度（弧度）

## 参考实现

参考 `/Volumes/DPC/work/plant-code/rvmparser/src/TriangulationFactory.cpp` 中的实现：
- `cylinder()` - 第 907-1000 行（作为拉伸体的参考）
- 旋转体的实现参考圆柱体的旋转生成逻辑

## 实现步骤

1. 在 `rs-core/src/geometry/csg.rs` 中找到 `generate_csg_mesh` 函数
2. 在 match 语句中添加两个新的分支
3. 将 `csg_extrusion_revolution_impl.rs` 中的函数复制到该文件
4. 实现每个基本体的顶点和索引生成逻辑
5. 返回 `Some(GeneratedMesh { ... })`

## 实现细节

### 拉伸体 (Extrusion)

拉伸体是将一个2D轮廓沿Z轴方向拉伸指定高度形成的3D实体：

1. **侧面生成**：
   - 沿高度方向分段（根据 LOD 设置）
   - 对轮廓的每条边，生成垂直于该边的法线
   - 创建侧面的四边形（两个三角形）

2. **端面生成**：
   - 底部面（z = -height/2）和顶部面（z = height/2）
   - 使用扇形三角剖分方法

3. **法线计算**：
   - 侧面法线：垂直于轮廓边，指向外部
   - 端面法线：底部 (-Z)，顶部 (+Z)

### 旋转体 (Revolution)

旋转体是将一个2D轮廓绕Z轴旋转指定角度形成的3D实体：

1. **侧面生成**：
   - 沿旋转角度方向分段（使用 sagitta 方法）
   - 对轮廓的每条边，计算旋转后的3D位置
   - 计算旋转后的法线方向

2. **端面生成**（如果角度 < 2π）：
   - 起始端面（angle = 0）
   - 结束端面（angle = angle）
   - 使用扇形三角剖分方法

3. **法线计算**：
   - 侧面法线：在轮廓平面内垂直于边，然后旋转到3D空间
   - 端面法线：垂直于旋转平面的方向

## 在 generate_csg_mesh 中添加实现

```rust
pub fn generate_csg_mesh(
    param: &PdmsGeoParam,
    csg_settings: &LodMeshSettings,
    non_scalable: bool,
) -> Option<GeneratedMesh> {
    match param {
        // ... 已有的基本体 ...
        
        PdmsGeoParam::PrimExtrusion(ext) => {
            generate_extrusion_mesh(
                &ext.verts,
                ext.height,
                csg_settings,
                non_scalable
            )
        }
        
        PdmsGeoParam::PrimRevolution(rev) => {
            generate_revolution_mesh(
                &rev.verts,
                rev.angle,
                csg_settings,
                non_scalable
            )
        }
        
        _ => None,
    }
}
```

## 注意事项

- 需要根据 LOD 设置计算合适的细分段数
- 法线计算要正确（确保光照正确）
- 确保顶点顺序符合右手定则（面朝向正确）
- 处理边界情况：
  - 零长度边
  - 空轮廓
  - 无效高度/角度
- 对于旋转体，需要处理角度 < 2π 时的端面生成
- AABB 计算需要考虑部分旋转的情况

## 测试建议

1. 简单矩形轮廓的拉伸体
2. 圆形轮廓的拉伸体
3. 复杂多边形的拉伸体
4. 完整旋转（2π）的旋转体
5. 部分旋转（< 2π）的旋转体
6. 带有内部轮廓的拉伸体（需要支持多个轮廓环）

## 相关文件

- 实现代码：`csg_extrusion_revolution_impl.rs`
- 参考实现：`/Volumes/DPC/work/plant-code/rvmparser/src/TriangulationFactory.cpp`
- 其他基本体实现：`csg_primitives_impl.rs`


