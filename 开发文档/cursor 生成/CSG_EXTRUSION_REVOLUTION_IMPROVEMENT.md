# CSG 拉伸体和旋转体实现改进建议

## 当前状态

`rs-core` 项目中已经实现了拉伸体和旋转体的 CSG 生成：

1. **`generate_extrusion_mesh`** (第 1856-1995 行)
   - ✅ 已实现基本功能
   - ❌ **缺少 LOD 支持**：未接受 `settings` 和 `non_scalable` 参数
   - ❌ 高度方向没有根据 LOD 设置进行细分

2. **`generate_revolution_mesh`** (第 2598-2797 行)
   - ✅ 已完整实现，包括 LOD 支持

## 改进建议

### 1. 更新 `generate_extrusion_mesh` 函数签名

**当前实现：**
```rust
fn generate_extrusion_mesh(extrusion: &Extrusion) -> Option<GeneratedMesh>
```

**建议改为：**
```rust
fn generate_extrusion_mesh(
    extrusion: &Extrusion,
    settings: &LodMeshSettings,
    non_scalable: bool,
) -> Option<GeneratedMesh>
```

### 2. 更新调用处

**当前调用（第 387 行）：**
```rust
PdmsGeoParam::PrimExtrusion(extrusion) => generate_extrusion_mesh(extrusion),
```

**应改为：**
```rust
PdmsGeoParam::PrimExtrusion(extrusion) => {
    generate_extrusion_mesh(extrusion, settings, non_scalable)
}
```

### 3. 在函数内部添加高度方向细分

在 `generate_extrusion_mesh` 函数中，可以根据 LOD 设置计算高度方向的细分段数：

```rust
// 计算高度方向的细分段数
let height_segments = compute_height_segments(
    settings,
    extrusion.height.abs(),
    non_scalable,
    1, // 最小分段数
);

// 如果高度分段数 > 1，需要在高度方向生成多个环
// 当前实现只在底部和顶部生成两个环，中间没有细分
```

### 4. 实现细节

当前实现仅在底部和顶部生成两个环，中间没有细分。如果需要根据 LOD 设置进行细分，可以：

1. 在高度方向生成多个环（`height_segments + 1` 个环）
2. 每个环都有完整的轮廓顶点
3. 相邻环之间生成侧面三角形

**注意**：如果高度不大，保持当前实现（只有两个环）也是可以接受的，因为拉伸体的侧面是平的，不需要太多细分。

## 实施步骤

1. **更新函数签名**：
   ```rust
   fn generate_extrusion_mesh(
       extrusion: &Extrusion,
       settings: &LodMeshSettings,
       non_scalable: bool,
   ) -> Option<GeneratedMesh>
   ```

2. **更新调用处**（第 387 行）：
   ```rust
   PdmsGeoParam::PrimExtrusion(extrusion) => {
       generate_extrusion_mesh(extrusion, settings, non_scalable)
   }
   ```

3. **（可选）添加高度方向细分**：
   - 如果高度较大，可以考虑在高度方向添加细分
   - 否则保持当前实现即可

## 测试建议

1. 测试不同高度的拉伸体
2. 测试不同 LOD 设置下的生成结果
3. 验证 AABB 计算正确性
4. 验证法线方向正确性

## 总结

- ✅ **旋转体**：已完整实现，无需修改
- ⚠️ **拉伸体**：需要添加 LOD 参数支持，但高度方向细分是可选的（因为侧面是平的）

当前实现已经可以正常工作，改进主要是为了保持 API 一致性（与其他基本体生成函数一致）。




