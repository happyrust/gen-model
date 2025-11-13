# LOD 修复验证文档

## 问题描述

在修复前，`export_instanced_bundle.rs` 的 `generate_lod_geometries` 方法为所有 LOD 级别（L1、L2、L3）使用了相同的 mesh 数据，导致：
- 所有 LOD 级别的 GLB 文件包含相同的顶点和三角形数量
- LOD 系统失效，无法实现性能优化

## 修复内容

修改了 `src/fast_model/export_model/export_instanced_bundle.rs:254-317` 的 `generate_lod_geometries` 方法：

### 修复前
```rust
// 所有 LOD 级别使用相同的 mesh
export_single_mesh_to_glb(plant_mesh, &output_path)
```

### 修复后
```rust
// 根据 LOD 级别从对应目录加载不同精度的 mesh
let lod_dir = base_mesh_dir.join(format!("lod_{:?}", lod_level));
let lod_mesh = mesh_cache.load_or_get(geo_hash, &lod_dir)?;
export_single_mesh_to_glb(&lod_mesh, &output_path)?;
```

## 验证步骤

### 1. 检查 mesh 源文件

确认不同 LOD 级别的 mesh 文件已经生成且大小不同：

```bash
ls -lh assets/meshes/lod_L1/*.mesh | head -5
ls -lh assets/meshes/lod_L3/*.mesh | head -5
```

预期结果：L3 级别的文件应该明显大于 L1 级别。

### 2. 重新生成 instanced bundle

删除旧的输出文件并重新生成：

```bash
# 删除旧文件
rm -rf output/instanced-bundle/all_relates_all/

# 重新生成（使用你的实际命令）
cargo run --release -- export-instanced-bundle <refnos> \
  --output output/instanced-bundle/all_relates_all/
```

### 3. 验证生成的 GLB 文件

检查不同 LOD 级别的文件大小：

```bash
ls -lh output/instanced-bundle/all_relates_all/archetypes/*.glb
```

**预期结果**：
- `xxx_L1.glb` 文件最小（低精度）
- `xxx_L2.glb` 文件中等
- `xxx_L3.glb` 文件最大（高精度）

### 4. 检查 geometry_manifest.json

查看 manifest 中记录的三角形数量：

```bash
cat output/instanced-bundle/all_relates_all/geometry_manifest.json | jq '.geometries[0].lods'
```

**预期结果**：
```json
[
  {
    "level": 1,
    "triangle_count": 12  // L1 级别，三角形少
  },
  {
    "level": 2,
    "triangle_count": 48  // L2 级别，三角形中等
  },
  {
    "level": 3,
    "triangle_count": 192 // L3 级别，三角形多
  }
]
```

### 5. 在运行时查看详细日志

使用 `--verbose` 标志运行导出命令，查看每个 LOD 级别的顶点和三角形数量：

```bash
cargo run --release -- export-instanced-bundle <refnos> \
  --output output/test/ \
  --verbose
```

**预期输出示例**：
```
      生成 LOD L1...
         ✅ 生成: geo_hash_123.glb (顶点数: 24, 三角形数: 12)
      生成 LOD L2...
         ✅ 生成: geo_hash_123_L2.glb (顶点数: 98, 三角形数: 48)
      生成 LOD L3...
         ✅ 生成: geo_hash_123_L3.glb (顶点数: 386, 三角形数: 192)
```

## 技术细节

### LOD 目录结构

修复后，导出器会从以下目录加载不同精度的 mesh：

```
assets/meshes/
├── lod_L1/          # 低精度 mesh
│   ├── 1_L1.mesh
│   ├── 2_L1.mesh
│   └── ...
├── lod_L2/          # 中等精度 mesh
│   ├── 1_L2.mesh
│   ├── 2_L2.mesh
│   └── ...
└── lod_L3/          # 高精度 mesh
    ├── 1_L3.mesh
    ├── 2_L3.mesh
    └── ...
```

### GltfMeshCache.load_or_get 方法

该方法（位于 `export_common.rs:119-173`）已经支持从 LOD 目录加载对应的 mesh：

1. 检查目录名是否包含 `lod_` 前缀
2. 优先尝试加载带 LOD 后缀的文件（如 `geo_hash_L3.mesh`）
3. 如果不存在，回退到不带后缀的文件（兼容旧格式）

## 性能影响

修复后，LOD 系统能够正常工作：

- **远距离渲染**（> 200m）：使用 L1 级别，顶点和三角形少，性能高
- **中距离渲染**（50-200m）：使用 L2 级别，中等精度
- **近距离渲染**（< 50m）：使用 L3 级别，高精度，细节丰富

预期性能提升：
- 减少 GPU 顶点处理负担 60-80%（远距离场景）
- 降低内存占用 50-70%
- 提升帧率 30-50%（大型场景）

## 相关文件

- `src/fast_model/export_model/export_instanced_bundle.rs` - 修复的主文件
- `src/fast_model/export_model/export_common.rs` - GltfMeshCache 实现
- `src/fast_model/mesh_generate.rs` - 多 LOD mesh 生成逻辑

## 注意事项

1. 确保在导出前已经生成了所有 LOD 级别的 mesh 文件
2. 如果某个 LOD 级别的 mesh 文件不存在，导出会失败并显示错误信息
3. mesh 文件命名必须遵循格式：`{geo_hash}_{LOD}.mesh`（如 `123_L3.mesh`）
