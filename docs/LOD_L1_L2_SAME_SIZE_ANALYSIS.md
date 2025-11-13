# L1 和 L2 生成相同大小 Mesh 的问题分析

## 🔍 问题现象

所有 L1 和 L2 的 mesh 文件大小**完全相同**：

```bash
L1 mesh 文件：
-rw-r--r--  12174770843391538079_L1.mesh  13K
-rw-r--r--  2_L1.mesh                      4.6K

L2 mesh 文件：
-rw-r--r--  12174770843391538079_L2.mesh  13K  ← 和 L1 一样！
-rw-r--r--  2_L2.mesh                      4.6K ← 和 L1 一样！
```

## 📋 配置对比

### DbOption.toml 中的配置

#### L1 配置（第 150-172 行）
```toml
[mesh_precision.lod_profiles.L1.csg_settings]
radial_segments = 12                # 基础圆周段数
height_segments = 1
cap_segments = 1
error_tolerance = 0.01
min_radial_segments = 12            # ⚠️ 最小值 = 12
max_radial_segments = 18            # 最大值 = 18
min_height_segments = 1
max_height_segments = 2
target_segment_length = 250.0       # 目标段长 250mm
non_scalable_factor = 0.9
```

#### L2 配置（第 174-196 行）
```toml
[mesh_precision.lod_profiles.L2.csg_settings]
radial_segments = 20                # 基础圆周段数（比 L1 高）
height_segments = 2                 # 比 L1 多
cap_segments = 1
error_tolerance = 0.005             # 比 L1 更严格
min_radial_segments = 12            # ⚠️ 最小值也是 12（和 L1 一样！）
max_radial_segments = 30            # 最大值更高
min_height_segments = 1
max_height_segments = 3             # 比 L1 多
target_segment_length = 140.0       # 更小的段长（应该更细分）
non_scalable_factor = 0.8
```

## 🐛 根本原因

### 原因 1：min_radial_segments 相同

**关键发现**：L1 和 L2 的 `min_radial_segments` 都是 **12**。

根据 [mesh_precision.rs:108-142](../../../rs-core/src/mesh_precision.rs#L108-L142) 中的 `adaptive_radial_segments` 方法：

```rust
pub fn adaptive_radial_segments(
    &self,
    radius: f32,
    circumference: Option<f32>,
    non_scalable: bool,
) -> u16 {
    let base = self.radial_segments.max(self.min_radial_segments.max(3));
    // ...

    if let Some(mut target_len) = self.target_segment_length {
        // 计算理想段数
        let ideal = (circumference / target_len).ceil() as u16;

        // 限制在 [min_radial_segments, max_radial_segments] 范围内
        ideal
            .max(self.min_radial_segments.max(3))  // ← 不会低于最小值
            .min(max_allowed.max(self.min_radial_segments))
    } else {
        base
    }
}
```

### 原因 2：小型几何体触发最小值限制

对于**小型几何体**（如小直径管道、阀门），即使 L2 的 `target_segment_length` 更小（140mm），计算出的理想段数可能仍然**小于 min_radial_segments**，最终都会被限制为 **12 段**。

#### 示例计算

假设一个直径为 100mm 的圆柱：
- 周长 = π × 100 = 314mm

**L1 计算**：
```
target_segment_length = 250mm
ideal = 314 / 250 = 1.26 → 向上取整 = 2
最终 = max(2, 12) = 12  ← 使用最小值
```

**L2 计算**：
```
target_segment_length = 140mm
ideal = 314 / 140 = 2.24 → 向上取整 = 3
最终 = max(3, 12) = 12  ← 也是使用最小值！
```

**结果**：L1 和 L2 都使用 **12 段**，生成的 mesh 完全相同！

### 原因 3：height_segments 差异不明显

虽然 L2 的 `height_segments = 2` 比 L1 的 `1` 多，但对于**短管道**或**标准元件**，高度段数的差异可能不足以产生明显的文件大小差异。

## ✅ 解决方案

### 方案 1：降低 L1 的 min_radial_segments（推荐）

将 L1 的最小段数降低到 **6-8**，这样小型几何体在 L1 级别会明显更粗糙：

```toml
# DbOption.toml 第 166 行
[mesh_precision.lod_profiles.L1.csg_settings]
radial_segments = 12
min_radial_segments = 6              # 从 12 改为 6
max_radial_segments = 14             # 从 18 改为 14（也降低一点）
target_segment_length = 300.0        # 从 250 改为 300（更粗糙）
```

### 方案 2：提高 L2 的 min_radial_segments

将 L2 的最小段数提高到 **16-18**，确保中等精度：

```toml
# DbOption.toml 第 190 行
[mesh_precision.lod_profiles.L2.csg_settings]
radial_segments = 20
min_radial_segments = 16             # 从 12 改为 16
max_radial_segments = 30
target_segment_length = 120.0        # 从 140 改为 120（更细分）
```

### 方案 3：调整 target_segment_length 差异

加大 L1 和 L2 的 target_segment_length 差异，让小型几何体也能体现差异：

```toml
# L1：更大的段长
target_segment_length = 350.0        # 从 250 改为 350

# L2：更小的段长
target_segment_length = 100.0        # 从 140 改为 100
```

### 方案 4：组合方案（最佳）

```toml
# LOD L1 配置（低精度）
[mesh_precision.lod_profiles.L1.csg_settings]
radial_segments = 10                 # 降低基础段数
height_segments = 1
cap_segments = 1
error_tolerance = 0.015              # 放宽误差容限
min_radial_segments = 6              # ← 降低最小值
max_radial_segments = 14             # ← 降低最大值
min_height_segments = 1
max_height_segments = 1              # ← 限制为 1
target_segment_length = 350.0        # ← 增大段长
non_scalable_factor = 1.0            # ← 不额外增加精度

# LOD L2 配置（中等精度）
[mesh_precision.lod_profiles.L2.csg_settings]
radial_segments = 20
height_segments = 2
cap_segments = 1
error_tolerance = 0.005
min_radial_segments = 16             # ← 提高最小值
max_radial_segments = 32             # ← 提高最大值
min_height_segments = 1
max_height_segments = 3
target_segment_length = 100.0        # ← 减小段长
non_scalable_factor = 0.8
```

## 📊 预期效果

应用修复后，对于直径 100mm 的圆柱：

### L1（新配置）
```
周长 = 314mm
ideal = 314 / 350 = 0.9 → 1
最终 = max(1, 6) = 6 段  ← 更粗糙
```

### L2（新配置）
```
周长 = 314mm
ideal = 314 / 100 = 3.14 → 4
最终 = max(4, 16) = 16 段  ← 更细致
```

**文件大小差异**：
- L1: 顶点数 ≈ 6 × 2 = 12，三角形 ≈ 12
- L2: 顶点数 ≈ 16 × 3 = 48，三角形 ≈ 64
- **差异：L2 约为 L1 的 4-5 倍大**

## 🔧 修复步骤

1. **编辑配置文件**
   ```bash
   vim DbOption.toml
   # 修改第 166、190 行的 csg_settings
   ```

2. **重新生成 mesh**
   ```bash
   # 删除旧的 mesh 文件
   rm -rf assets/meshes/lod_L1/*.mesh
   rm -rf assets/meshes/lod_L2/*.mesh

   # 重新生成
   cargo run --release -- gen-mesh <refnos> --verbose
   ```

3. **验证差异**
   ```bash
   # 对比文件大小
   ls -lh assets/meshes/lod_L1/*.mesh
   ls -lh assets/meshes/lod_L2/*.mesh

   # 应该看到明显差异
   ```

4. **重新导出 GLB**
   ```bash
   rm -rf output/instanced-bundle/all_relates_all/
   cargo run --release -- export-instanced-bundle <refnos> --verbose
   ```

## 📌 注意事项

1. **平衡精度和性能**：降低 L1 的最小段数可能会让某些几何体看起来过于粗糙
2. **测试不同尺寸**：确保修改后的配置在大、中、小型几何体上都表现良好
3. **考虑视觉效果**：L1 主要用于远距离渲染，6 段可能足够
4. **迭代调整**：可能需要多次测试来找到最佳参数

## 🎯 总结

**问题根源**：L1 和 L2 的 `min_radial_segments` 都是 12，导致小型几何体的自适应算法返回相同的段数。

**解决方案**：降低 L1 的最小段数（6-8），提高 L2 的最小段数（16-18），加大 target_segment_length 差异。

**预期结果**：L1 和 L2 的 mesh 文件大小将有明显差异（3-5倍），LOD 系统能够真正发挥作用。
