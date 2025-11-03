# XKT 索引问题分析报告

## 问题症状

```
[.WebGL-0x12400535000] GL_INVALID_OPERATION: glDrawElements: Vertex buffer is not big enough for the draw call.
```

## 根本原因分析

### 1. 问题定位

调试脚本显示**所有 8 个几何体都有索引违规**：

```
Geometry 0: 69 vertices, 最大索引 137 (违规 69)
Geometry 1: 323 vertices, 最大索引 645 (违规 323)  
Geometry 2: 12 vertices, 最大索引 23 (违规 12)
Geometry 3: 116 vertices, 最大索引 231 (违规 116)
...
```

**关键发现**：每个几何体的最大索引值约等于其顶点数的 **2 倍**。

### 2. 数据流程分析

#### ✅ PlantMesh 数据是正确的
- 从 `../rs-core/src/shape/pdms_shape.rs` 看到，`PlantMesh` 的 `indices` 是**局部索引**（相对于该几何体的 vertices）
- 每个几何体的索引范围应为 `[0, vertexCount-1]`

#### ❌ XKT 导出过程有问题

在 `src/fast_model/export_xkt.rs:304-309`:

```rust
xkt_geometry.positions = self.flatten_vec3(&plant_mesh.vertices);
xkt_geometry.normals = Some(self.flatten_vec3(&plant_mesh.normals));
xkt_geometry.indices = plant_mesh.indices.clone();
```

**问题**：代码注释说"索引已经相对于 geometry 的顶点偏移量"，但事实不是这样！

### 3. gen-xkt 库的行为

XKT v12 格式：
- 所有几何体的顶点合并到一个全局 POSITIONS 数组
- 每个几何体的索引必须**相对于全局顶点数组中的起始位置**
- 或者使用**局部索引 + 顶点偏移**

当前实现：
- ❌ 直接复制了 PlantMesh 的索引（局部索引 0..N-1）
- ❌ 但顶点被合并到全局数组（位置被偏移了）
- 导致索引引用错误位置

## 解决方案

### 方案 A：调整索引（推荐）

在创建 XKT Geometry 时，需要调整索引以匹配全局顶点偏移：

```rust
// 计算全局顶点偏移
let vertex_offset = /* 累加的前面所有几何体的顶点数 */;

// 调整索引
let adjusted_indices: Vec<u32> = plant_mesh.indices.iter()
    .map(|&idx| idx + vertex_offset)
    .collect();
    
xkt_geometry.indices = adjusted_indices;
```

### 方案 B：使用独立顶点数组（简单但不高效）

每个几何体保留独立的顶点数组，不合并（减少内存复用）。

### 方案 C：修复 gen-xkt 库

检查 gen-xkt 库是否正确处理了索引偏移。如果库应该自动处理但没处理，需要修复库或升级版本。

## 建议的修复步骤

1. **立即修复**：在 `export_xkt.rs` 的 `create_unique_geometries` 函数中，累加顶点偏移并调整索引

2. **验证**：重新生成 XKT 文件并运行验证脚本

3. **测试**：在 Web 查看器中确认模型正确渲染

## 临时解决方案

如果需要在 XKT v12 中工作，可以考虑：
- 每个几何体使用独立的顶点数组（增加文件大小）
- 或者降级到 XKT v10/v11（如果支持更好的索引处理）



