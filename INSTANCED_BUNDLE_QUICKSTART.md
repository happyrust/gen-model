# Instanced Bundle 快速开始

## 5 分钟上手指南

### 步骤 1: 导出数据（后端）

```bash
cd gen-model

# 导出测试数据
cargo run --bin gen-model -- export \
  --format instanced-bundle \
  --refnos "21491/18946" \
  --output output/instanced-bundle/test \
  --verbose
```

**预期输出**：

```
🎯 Instanced Bundle 导出模式
================
   - 参考号数量: 1
   
🚀 开始导出 Instanced Bundle...
   ✅ 创建目录结构完成
   
📦 收集元件实例数据...
   ✅ 收集到 45 个唯一几何体
   ✅ 总实例数: 1523

🔨 为每个 geo_hash 生成 LOD 几何体...
   [处理进度...]

✅ Manifest 文件写入完成
   - 总 archetype 数: 45
   - 总实例数: 1523

🎉 Instanced Bundle 导出完成！
   输出目录: output/instanced-bundle/test
```

### 步骤 2: 查看导出结果

```bash
# 查看文件结构
tree output/instanced-bundle/test

# 输出示例：
# output/instanced-bundle/test/
# ├── manifest.json
# ├── archetypes/
# │   ├── abc123.glb
# │   ├── abc123_L2.glb
# │   ├── abc123_L3.glb
# │   └── ...
# └── instances/
#     ├── abc123.json
#     └── ...
```

### 步骤 3: 前端加载（可选）

```bash
cd ../instanced-mesh

# 安装依赖（首次运行）
npm install

# 启动开发服务器
npm run dev

# 浏览器打开
# http://localhost:5173/examples/aios-instanced-loader.html
```

**注意**: 确保将后端导出的 `output/instanced-bundle/` 目录复制到前端项目的 `public/` 目录下。

### 步骤 4: 验证加载

打开浏览器控制台，应该看到：

```
🚀 加载 Instanced Bundle...
   Manifest URL: output/instanced-bundle/manifest.json
   
📦 Manifest 加载完成:
   - 版本: 1.0
   - 总 archetypes: 45
   - 总实例: 1523
   
✅ 加载完成！
   - 成功加载 45/45 个 archetypes
   - 成功加载 1523/1523 个实例
   - 总三角形数: 456,789
```

## 常见场景

### 场景 1: 导出整个 ZONE

```bash
# 导出包含所有子对象的 ZONE
cargo run --bin gen-model -- export \
  --format instanced-bundle \
  --refnos "21491/100" \
  --include-descendants \
  --output output/zones/zone_100
```

### 场景 2: 仅导出特定类型

```bash
# 仅导出管道和阀门
cargo run --bin gen-model -- export \
  --format instanced-bundle \
  --refnos "21491/100" \
  --filter-nouns "PIPE,VALVE,ELBO,TEE" \
  --output output/piping/zone_100
```

### 场景 3: 批量导出多个对象

```bash
# 导出多个 refno
cargo run --bin gen-model -- export \
  --format instanced-bundle \
  --refnos "21491/100,21491/101,21491/102" \
  --output output/multi/zones
```

## 性能优化技巧

### 1. 根据场景调整 LOD 距离

**小场景**（< 100m）：

```rust
// export_instanced_bundle.rs
const LOD_DISTANCES: &[f32] = &[0.0, 20.0, 80.0];
```

**大场景**（> 1000m）：

```rust
const LOD_DISTANCES: &[f32] = &[0.0, 100.0, 500.0];
```

### 2. 前端性能调优

```typescript
// 禁用不必要的功能以提升性能
const instancedMesh = new InstancedMesh2(
  geometry,
  material,
  {
    capacity: count,
    createEntities: false,  // 不创建实体（节省内存）
  }
);

// 使用简化的 BVH 参数
instancedMesh.computeBVH({
  margin: 0,
  getBBoxFromBSphere: true,  // 使用球体近似（更快）
  accurateCulling: false     // 简化剔除（更快）
});
```

### 3. 分批加载大场景

```typescript
// 按 archetype 分批加载
for (const archetype of manifest.archetypes) {
  await loadArchetype(archetype);
  
  // 每加载 10 个就渲染一次，避免卡顿
  if (++loadedCount % 10 === 0) {
    await new Promise(resolve => requestAnimationFrame(resolve));
  }
}
```

## 疑难解答

### Q: 导出的 GLB 文件很大怎么办？

**A**: 使用更粗糙的 LOD 级别：

```toml
# DbOption.toml
[mesh_precision.lod_profiles.L1]
radial_segments = 8  # 降低段数
```

### Q: 前端加载很慢怎么办？

**A**: 
1. 检查网络：确保使用本地服务器而非 file:// 协议
2. 减少 LOD 级别：只使用 L1 和 L2
3. 启用 gzip 压缩：服务器配置 gzip 压缩 .glb 和 .json 文件

### Q: LOD 切换有闪烁怎么办？

**A**: 增加 LOD hysteresis（滞后）：

```typescript
instancedMesh.addLOD(
  lodGeometry,
  material,
  distance,
  0.1  // hysteresis = 10%
);
```

## 下一步

- 📖 阅读[完整文档](./INSTANCED_BUNDLE_README.md)
- 🎨 查看[示例代码](../instanced-mesh/examples/aios-instanced-loader.ts)
- 🚀 尝试[性能优化](./INSTANCED_BUNDLE_README.md#性能优化建议)

## 获取帮助

遇到问题？

1. 检查控制台输出的错误信息
2. 查看[故障排查](./INSTANCED_BUNDLE_README.md#故障排查)章节
3. 提交 Issue 到项目仓库


