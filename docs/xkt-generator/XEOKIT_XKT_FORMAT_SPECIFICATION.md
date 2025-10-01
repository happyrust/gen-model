# xeokit XKT 格式规范详解

## 概述

基于对 xeokit 官方文档的深入研究，本文档详细描述了 xeokit XKT V4.0 格式规范，为实现标准兼容的 XKT 生成器提供技术基础。

## 1. XKT 格式版本对比

### 当前项目 vs xeokit 标准

| 特性 | 当前实现 | xeokit XKT V4.0 标准 |
|------|----------|---------------------|
| 版本号 | 自定义版本 10 | 标准版本 4 |
| 文件头 | 24字节自定义头 | 标准化索引结构 |
| 几何压缩 | 基础压缩 | 16位量化 + 8位法向量编码 |
| 几何复用 | 无复用 | 支持几何实例化 |
| 坐标系统 | 简单世界坐标 | 世界坐标 + 模型坐标混合 |
| 解码矩阵 | 无 | K-d树分区量化解码 |

## 2. XKT V4.0 文件结构

### 2.1 文件头和索引结构

```
XKT V4.0 File Structure:
┌─────────────────────────────┐
│ version (Uint32)            │ ← 值必须为 4
├─────────────────────────────┤
│ size_index (Uint32)         │ ← 索引区域大小
├─────────────────────────────┤
│ Index Section:              │
│  - size_positions           │
│  - size_normals             │
│  - size_indices             │
│  - size_edge_indices        │
│  - size_decode_matrices     │
│  - size_each_primitive_*    │
│  - size_primitive_instances │
│  - size_each_entity_*       │
├─────────────────────────────┤
│ Data Section (zlib压缩):    │
│  - positions (Uint16[])     │
│  - normals (Uint8[])        │
│  - indices (Uint32[])       │
│  - edge_indices (Uint32[])  │
│  - decode_matrices (Float32[])│
│  - primitive metadata      │
│  - entity metadata         │
└─────────────────────────────┘
```

### 2.2 关键数据结构

#### 几何数据压缩
```rust
// 位置数据：32位浮点 → 16位整数
positions: Vec<u16>  // 量化后的顶点位置

// 法向量：32位浮点 → 8位整数  
normals: Vec<u8>     // Oct编码的法向量

// 解码矩阵：用于反量化
decode_matrices: Vec<f32>  // 16元素变换矩阵
```

#### 几何复用系统
```rust
// 基元（可复用的几何体）
struct Primitive {
    positions_portion: u32,    // 在positions数组中的起始索引
    normals_portion: u32,      // 在normals数组中的起始索引
    indices_portion: u32,      // 在indices数组中的起始索引
    decode_matrix_portion: u32, // 解码矩阵索引
    color: [u8; 4],           // RGBA颜色
}

// 基元实例（实体对基元的引用）
struct PrimitiveInstance {
    primitive_id: u32,         // 引用的基元ID
    matrix: [f32; 16],        // 实例变换矩阵
}

// 实体（逻辑对象）
struct Entity {
    id: String,                        // 实体ID
    primitive_instances_portion: u32,   // 实例数组起始索引
    matrix: [f32; 16],                // 实体变换矩阵
}
```

## 3. 几何数据处理

### 3.1 位置量化算法

xeokit 使用 K-d 树分区量化来减少精度损失：

```rust
// 量化过程
fn quantize_positions(positions: &[f32]) -> (Vec<u16>, Vec<f32>) {
    // 1. 使用K-d树将位置分割为子区域
    let regions = kd_tree_partition(positions);
    
    let mut quantized = Vec::new();
    let mut decode_matrices = Vec::new();
    
    for region in regions {
        // 2. 计算每个区域的边界框
        let (min, max) = calculate_bounds(&region.positions);
        
        // 3. 生成解码矩阵
        let decode_matrix = create_decode_matrix(min, max);
        decode_matrices.extend_from_slice(&decode_matrix);
        
        // 4. 量化到16位范围
        for pos in region.positions.chunks(3) {
            let quantized_pos = quantize_to_u16(pos, min, max);
            quantized.extend_from_slice(&quantized_pos);
        }
    }
    
    (quantized, decode_matrices)
}

fn create_decode_matrix(min: Vec3, max: Vec3) -> [f32; 16] {
    let scale = (max - min) / 65535.0; // 16位最大值
    [
        scale.x, 0.0, 0.0, min.x,
        0.0, scale.y, 0.0, min.y,
        0.0, 0.0, scale.z, min.z,
        0.0, 0.0, 0.0, 1.0
    ]
}
```

### 3.2 法向量 Oct 编码

```rust
// Oct编码：将3D法向量编码为2个8位值
fn oct_encode_normal(normal: Vec3) -> [u8; 2] {
    let n = normal / (normal.x.abs() + normal.y.abs() + normal.z.abs());
    
    let (x, y) = if n.z >= 0.0 {
        (n.x, n.y)
    } else {
        let sign_x = if n.x >= 0.0 { 1.0 } else { -1.0 };
        let sign_y = if n.y >= 0.0 { 1.0 } else { -1.0 };
        ((1.0 - n.y.abs()) * sign_x, (1.0 - n.x.abs()) * sign_y)
    };
    
    [
        ((x * 0.5 + 0.5) * 255.0) as u8,
        ((y * 0.5 + 0.5) * 255.0) as u8,
    ]
}
```

### 3.3 边缘索引生成

```rust
// 为线框渲染生成边缘索引
fn generate_edge_indices(indices: &[u32]) -> Vec<u32> {
    let mut edges = std::collections::HashSet::new();
    
    // 从三角形索引提取边
    for triangle in indices.chunks(3) {
        let edges_in_triangle = [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ];
        
        for (a, b) in edges_in_triangle {
            let edge = if a < b { (a, b) } else { (b, a) };
            edges.insert(edge);
        }
    }
    
    // 转换为边缘索引数组
    edges.into_iter().flat_map(|(a, b)| [a, b]).collect()
}
```

## 4. 坐标系统和变换

### 4.1 世界坐标 vs 模型坐标

xeokit 使用混合坐标系统优化性能：

- **世界坐标**：仅被一个实体使用的几何体
- **模型坐标**：被多个实体共享的几何体

```rust
enum CoordinateSpace {
    World,  // 直接在世界空间中的位置
    Model,  // 需要通过实体矩阵变换的位置
}

struct GeometryPlacement {
    space: CoordinateSpace,
    positions: Vec<u16>,
    transform_matrix: Option<[f32; 16]>, // 仅模型坐标需要
}
```

### 4.2 变换矩阵层次

```rust
// 完整的变换链
final_position = entity_matrix * primitive_instance_matrix * model_position
```

## 5. 数据压缩策略

### 5.1 zlib 压缩

所有几何数据和元数据都使用 zlib 压缩：

```rust
use flate2::{Compression, write::ZlibEncoder};

fn compress_data(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}
```

### 5.2 数据布局优化

- 相同类型数据连续存储
- 索引数据分离存储
- 元数据JSON字符串压缩

## 6. 与当前实现的主要差异

### 6.1 文件格式差异

| 组件 | 当前实现 | xeokit 标准 |
|------|----------|-------------|
| 魔数 | "XKT\0" | 版本号4 |
| 头部 | 固定24字节 | 动态索引结构 |
| 压缩 | 可选gzip | 必需zlib |
| 几何体 | 简单三角网格 | 量化+Oct编码 |

### 6.2 性能优化差异

| 特性 | 当前实现 | xeokit 标准 |
|------|----------|-------------|
| 文件大小 | 较大 | 高度压缩 |
| 加载速度 | 中等 | 极快 |
| 内存使用 | 较高 | 优化 |
| GPU友好 | 一般 | 高度优化 |

这个规范为实现标准兼容的 xeokit XKT 生成器提供了完整的技术基础。
