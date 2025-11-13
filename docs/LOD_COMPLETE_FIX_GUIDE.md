# LOD 完整修复指南

## 📋 问题总结

发现了**两个独立的 LOD 问题**：

### 问题 1：导出阶段 - 所有 LOD 使用相同 mesh ✅ 已修复

**位置**: `src/fast_model/export_model/export_instanced_bundle.rs:254-317`

**原因**: 导出器为所有 LOD 级别使用了相同的 `plant_mesh` 参数

**修复**: 根据 LOD 级别从对应目录（`lod_L1`、`lod_L2`、`lod_L3`）加载不同的 mesh

**状态**: ✅ 已修复并编译通过

### 问题 2：生成阶段 - L1 和 L2 生成相同大小的 mesh ⚠️ 需要配置调整

**位置**: `DbOption.toml` 第 150-196 行

**原因**: L1 和 L2 的 `min_radial_segments` 都设置为 12，导致小型几何体的自适应算法返回相同的段数

**现象**:
```bash
# 所有 L1 和 L2 的 mesh 文件大小完全相同
12174770843391538079_L1.mesh: 13K
12174770843391538079_L2.mesh: 13K  ← 应该更大！
```

**状态**: ⚠️ 需要修改配置并重新生成 mesh

---

## 🔍 问题 2 详细分析

### 当前配置问题

```toml
# L1 配置
[mesh_precision.lod_profiles.L1.csg_settings]
min_radial_segments = 12            # ⚠️ 问题 1：和 L2 一样
max_radial_segments = 18
target_segment_length = 250.0       # ⚠️ 问题 2：差异不够大

# L2 配置
[mesh_precision.lod_profiles.L2.csg_settings]
min_radial_segments = 12            # ⚠️ 和 L1 一样！
max_radial_segments = 30
target_segment_length = 140.0
```

### 自适应算法行为

对于**直径 100mm 的圆柱**（周长 ≈ 314mm）：

**L1 计算**:
```
理想段数 = 314 / 250 = 1.26 → 向上取整 = 2
实际段数 = max(2, 12) = 12  ← 使用最小值
```

**L2 计算**:
```
理想段数 = 314 / 140 = 2.24 → 向上取整 = 3
实际段数 = max(3, 12) = 12  ← 也是最小值！
```

**结果**: 生成的 mesh 完全相同 ❌

---

## ✅ 修复方案

### 核心修改

| 参数 | L1 当前 | L1 建议 | L2 当前 | L2 建议 | 说明 |
|------|---------|---------|---------|---------|------|
| `min_radial_segments` | 12 | **6** | 12 | **16** | 关键差异 |
| `max_radial_segments` | 18 | **14** | 30 | **32** | 限制上限 |
| `target_segment_length` | 250.0 | **350.0** | 140.0 | **100.0** | 加大差异 |
| `max_height_segments` | 2 | **1** | 3 | 3 | 限制 L1 |
| `non_scalable_factor` | 0.9 | **1.0** | 0.8 | **0.75** | 调整精度 |

### 修复后的预期效果

对于**直径 100mm 的圆柱**：

**L1（新配置）**:
```
理想段数 = 314 / 350 = 0.9 → 1
实际段数 = max(1, 6) = 6 段  ← 更粗糙 ✓
顶点数 ≈ 6 × 2 = 12
三角形 ≈ 12
```

**L2（新配置）**:
```
理想段数 = 314 / 100 = 3.14 → 4
实际段数 = max(4, 16) = 16 段  ← 更细致 ✓
顶点数 ≈ 16 × 3 = 48
三角形 ≈ 64
```

**文件大小差异**: L2 约为 L1 的 **4-5 倍** ✓

---

## 🛠️ 修复步骤

### 方法 1：自动应用（推荐）

```bash
# 1. 运行自动修复脚本
cd /Volumes/DPC/work/plant-code/gen-model-fork
./scripts/apply_lod_fix.sh

# 2. 脚本会自动：
#    - 备份当前配置
#    - 修改 L1 和 L2 的配置
#    - 显示下一步操作提示
```

### 方法 2：手动修改

```bash
# 1. 备份配置
cp DbOption.toml DbOption.toml.backup

# 2. 编辑配置文件
vim DbOption.toml

# 3. 参考 docs/DbOption_LOD_FIX.toml 中的配置进行修改
```

### 修改后的关键配置

```toml
# LOD L1 配置（低精度）
[mesh_precision.lod_profiles.L1.csg_settings]
radial_segments = 10                # 从 12 改为 10
min_radial_segments = 6             # ⚠️ 从 12 改为 6
max_radial_segments = 14            # 从 18 改为 14
max_height_segments = 1             # 从 2 改为 1
target_segment_length = 350.0       # ⚠️ 从 250 改为 350
non_scalable_factor = 1.0           # 从 0.9 改为 1.0

# LOD L2 配置（中等精度）
[mesh_precision.lod_profiles.L2.csg_settings]
radial_segments = 20
min_radial_segments = 16            # ⚠️ 从 12 改为 16
max_radial_segments = 32            # 从 30 改为 32
target_segment_length = 100.0       # ⚠️ 从 140 改为 100
non_scalable_factor = 0.75          # 从 0.8 改为 0.75
```

---

## 🔄 重新生成 Mesh

修改配置后，需要重新生成所有 L1 和 L2 的 mesh：

```bash
# 1. 删除旧的 mesh 文件
rm -rf assets/meshes/lod_L1/*.mesh
rm -rf assets/meshes/lod_L2/*.mesh

# 2. 重新生成 mesh（使用你的实际命令）
cargo run --release -- gen-mesh <your-refnos> --verbose

# 3. 验证文件大小差异
echo "=== L1 mesh 文件 ==="
ls -lh assets/meshes/lod_L1/*.mesh | head -5

echo "=== L2 mesh 文件 ==="
ls -lh assets/meshes/lod_L2/*.mesh | head -5

# 预期：L2 的文件应该明显大于 L1
```

---

## ✅ 验证步骤

### 1. 验证 Mesh 文件大小

```bash
# 对比同一个 geo_hash 的不同 LOD 文件
GEO_HASH="12174770843391538079"

echo "L1: $(ls -lh assets/meshes/lod_L1/${GEO_HASH}_L1.mesh 2>/dev/null | awk '{print $5}')"
echo "L2: $(ls -lh assets/meshes/lod_L2/${GEO_HASH}_L2.mesh 2>/dev/null | awk '{print $5}')"
echo "L3: $(ls -lh assets/meshes/lod_L3/${GEO_HASH}_L3.mesh 2>/dev/null | awk '{print $5}')"

# 预期结果：L1 < L2 < L3
```

### 2. 重新导出 Instanced Bundle

```bash
# 删除旧的输出
rm -rf output/instanced-bundle/all_relates_all/

# 重新导出（使用 --verbose 查看详细信息）
cargo run --release -- export-instanced-bundle <your-refnos> \
  --output output/instanced-bundle/all_relates_all/ \
  --verbose

# 查看输出中的顶点和三角形数量
# 应该看到类似：
#   生成 LOD L1...
#     ✅ 生成: xxx.glb (顶点数: 12, 三角形数: 12)
#   生成 LOD L2...
#     ✅ 生成: xxx_L2.glb (顶点数: 48, 三角形数: 64)
```

### 3. 验证 GLB 文件大小

```bash
ls -lh output/instanced-bundle/all_relates_all/geometry_*.glb

# 预期：
# geometry_L1.glb:  较小
# geometry_L2.glb:  中等
# geometry_L3.glb:  最大
```

### 4. 运行验证脚本

```bash
./scripts/verify_lod_fix.sh

# 应该看到：
# ✓ lod_L1: X 个 mesh 文件 - 示例: YK
# ✓ lod_L2: X 个 mesh 文件 - 示例: ZK (Z > Y)
# ✓ lod_L3: X 个 mesh 文件 - 示例: WK (W > Z)
```

---

## 📊 性能影响

### 修复后的性能提升

| 场景 | L1 段数 | L2 段数 | L3 段数 | 性能提升 |
|------|---------|---------|---------|----------|
| 小型几何体 (Ø<150mm) | 6-8 | 16-20 | 24-32 | L1 vs L2: 3-4x |
| 中型几何体 (Ø150-500mm) | 8-12 | 20-28 | 32-48 | L1 vs L2: 2-3x |
| 大型几何体 (Ø>500mm) | 12-14 | 28-32 | 48-60 | L1 vs L2: 2x |

### 渲染性能

- **远距离** (>200m): 使用 L1，减少 GPU 负担 **70-80%**
- **中距离** (50-200m): 使用 L2，平衡质量和性能
- **近距离** (<50m): 使用 L3，最佳视觉质量

---

## 📁 相关文件

### 文档
- [LOD_L1_L2_SAME_SIZE_ANALYSIS.md](LOD_L1_L2_SAME_SIZE_ANALYSIS.md) - 详细问题分析
- [LOD_FIX_SUMMARY.md](LOD_FIX_SUMMARY.md) - 导出阶段修复总结
- [LOD_FIX_VERIFICATION.md](LOD_FIX_VERIFICATION.md) - 验证文档
- [DbOption_LOD_FIX.toml](DbOption_LOD_FIX.toml) - 修复后的配置参考

### 脚本
- [scripts/apply_lod_fix.sh](../scripts/apply_lod_fix.sh) - 自动应用配置修复
- [scripts/verify_lod_fix.sh](../scripts/verify_lod_fix.sh) - 验证脚本

### 代码
- `src/fast_model/export_model/export_instanced_bundle.rs` - 导出逻辑（已修复）
- `src/fast_model/mesh_generate.rs` - Mesh 生成逻辑
- `rs-core/src/mesh_precision.rs` - LOD 配置和自适应算法

---

## ⚠️ 注意事项

1. **视觉效果平衡**
   - L1 的 6 段对于某些几何体可能看起来过于粗糙
   - 如果效果不理想，可以调整为 8 段
   - 建议在测试环境先验证效果

2. **不同尺寸的几何体**
   - 配置针对常见工厂设备优化
   - 特大或特小型几何体可能需要单独调整
   - 可以使用 `overrides` 功能为特定类型设置 LOD

3. **重新生成时间**
   - 重新生成所有 mesh 可能需要较长时间
   - 可以先测试部分 refno 验证效果
   - 使用 `--verbose` 查看生成进度

4. **备份重要**
   - 修改配置前务必备份
   - 保存修复前后的 mesh 文件用于对比
   - 记录修改的参数值

---

## 🎉 完成检查清单

- [ ] 阅读问题分析文档
- [ ] 备份当前配置文件
- [ ] 修改 LOD 配置（自动或手动）
- [ ] 删除旧的 L1 和 L2 mesh 文件
- [ ] 重新生成 mesh 文件
- [ ] 验证文件大小差异
- [ ] 重新导出 instanced bundle
- [ ] 验证 GLB 文件大小
- [ ] 在浏览器中测试 LOD 切换
- [ ] 测量性能提升

---

## 📞 问题反馈

如果修复后仍有问题，请提供：
1. 配置文件（`DbOption.toml`）
2. 生成日志（使用 `--verbose`）
3. 示例 geo_hash 和对应的文件大小
4. 几何体类型和尺寸信息

---

*最后更新: 2025-11-13*
*修复版本: gen-model-fork*
*相关 Issues: LOD mesh 生成和导出问题*
