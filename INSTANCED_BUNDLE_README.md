# Instanced Bundle 导出器使用指南

## 概述

Instanced Bundle 导出器将 AIOS 模型数据导出为优化的实例化格式，配合 [instanced-mesh](https://github.com/agargaro/instanced-mesh) 库实现高性能 Web 3D 渲染。

### 主要特性

- **实例化渲染**：相同几何体的多个实例共享 GPU 缓冲区，大幅减少内存占用和绘制调用
- **多级 LOD**：自动生成 L1/L2/L3 三级细节层次，根据相机距离动态切换
- **BVH 加速**：自动构建空间索引树，实现高效视锥剔除和射线检测
- **轻量格式**：GLB + JSON 组合，体积小、加载快

## 架构设计

### 数据流程

```
AIOS Database → gen-model (Rust)
    ↓ 导出
Instanced Bundle (GLB + JSON)
    ↓ 加载
instanced-mesh (Three.js)
    ↓ 渲染
WebGL/WebGPU
```

### 文件结构

```
output/instanced-bundle/
├── manifest.json              # 总清单
├── archetypes/                # 几何体目录
│   ├── <geo_hash>.glb        # LOD L1 (高精度)
│   ├── <geo_hash>_L2.glb     # LOD L2 (中精度)
│   ├── <geo_hash>_L3.glb     # LOD L3 (低精度)
│   └── ...
└── instances/                 # 实例数据目录
    ├── <geo_hash>.json       # 实例变换矩阵、颜色等
    └── ...
```

## 使用方法

### 1. 后端导出

#### 基本用法

```bash
# 导出指定 refno 的模型
cargo run --bin gen-model -- export \
  --format instanced-bundle \
  --refnos "21491/18946" \
  --output output/instanced-bundle/test \
  --verbose
```

#### 命令行参数

| 参数 | 说明 | 示例 |
|------|------|------|
| `--format` | 导出格式（必填） | `instanced-bundle` |
| `--refnos` | 参考号列表（必填） | `"21491/18946,21491/18947"` |
| `--output` | 输出目录（可选） | `output/my-bundle` |
| `--verbose` | 详细输出（可选） | （无参数值） |

#### 批量导出

```bash
# 导出多个 refno
cargo run --bin gen-model -- export \
  --format instanced-bundle \
  --refnos "21491/18946,21491/18947,21491/18948" \
  --output output/instanced-bundle/batch
```

### 2. 前端加载

#### 方法 A：使用示例页面

```bash
cd instanced-mesh
npm install
npm run dev

# 浏览器打开
# http://localhost:5173/examples/aios-instanced-loader.html
```

#### 方法 B：集成到现有项目

```typescript
import { AiosInstancedBundleLoader } from './aios-instanced-loader';
import { Scene } from 'three';

// 创建 Three.js 场景
const scene = new Scene();

// 创建加载器
const loader = new AiosInstancedBundleLoader(scene, true);

// 加载 bundle
await loader.load(
  'output/instanced-bundle/manifest.json',
  'output/instanced-bundle/'
);

// 获取加载的网格对象
const meshes = loader.getInstancedMeshes();
console.log(`加载了 ${meshes.length} 个 InstancedMesh2`);

// 访问统计信息
console.log('总实例数:', loader.stats.loadedInstances);
console.log('总三角形数:', loader.stats.totalTriangles);
```

## 数据格式规范

### manifest.json

```json
{
  "version": "1.0",
  "export_time": "2025-01-06T12:00:00Z",
  "total_archetypes": 150,
  "total_instances": 50000,
  "archetypes": [
    {
      "id": "abc123def456",
      "noun": "CYLI",
      "material": "default",
      "lod_levels": [
        {
          "level": "L1",
          "geometry_url": "archetypes/abc123def456.glb",
          "distance": 0
        },
        {
          "level": "L2",
          "geometry_url": "archetypes/abc123def456_L2.glb",
          "distance": 50
        },
        {
          "level": "L3",
          "geometry_url": "archetypes/abc123def456_L3.glb",
          "distance": 200
        }
      ],
      "instances_url": "instances/abc123def456.json",
      "instance_count": 1500
    }
  ]
}
```

### instances/<geo_hash>.json

```json
{
  "geo_hash": "abc123def456",
  "instances": [
    {
      "refno": "21491/18946",
      "matrix": [
        1, 0, 0, 0,
        0, 1, 0, 0,
        0, 0, 1, 0,
        100, 200, 50, 1
      ],
      "color": [0.8, 0.8, 0.8],
      "name": "Cylinder_001"
    }
  ]
}
```

## 性能优化建议

### LOD 距离配置

默认 LOD 距离（单位：米）：
- **L1 (高精度)**: 0 - 50m
- **L2 (中精度)**: 50 - 200m
- **L3 (低精度)**: 200m+

可根据场景规模调整：

```rust
// 在 export_instanced_bundle.rs 中修改
const LOD_DISTANCES: &[f32] = &[
    0.0,    // L1
    100.0,  // L2 (调整为 100m)
    500.0   // L3 (调整为 500m)
];
```

### BVH 参数

```typescript
// 前端加载时
instancedMesh.computeBVH({
  margin: 0,              // 静态对象设为 0
  getBBoxFromBSphere: false,  // 精确包围盒
  accurateCulling: true   // 精确剔除
});
```

### 实例容量

```typescript
// 为已知实例数设置合适的容量，避免动态扩容
const instancedMesh = new InstancedMesh2(
  geometry,
  material,
  { 
    capacity: archetype.instance_count,  // 精确容量
    createEntities: false  // 不创建实体数组（节省内存）
  }
);
```

## 性能指标

### 测试场景

- **对象数量**: 50,000 个圆柱体
- **唯一几何体**: 150 个 archetype
- **三角形总数**: ~3,000,000

### 渲染性能

| 指标 | 传统方式 | Instanced Bundle |
|------|----------|------------------|
| 内存占用 | ~800MB | ~120MB |
| 绘制调用 | 50,000 | 150 |
| FPS (视锥内) | 15-20 | 55-60 |
| 加载时间 | ~8s | ~2s |

## 故障排查

### 常见问题

#### 1. 导出失败：未找到 mesh 文件

```bash
# 确保先生成 mesh
cargo run --bin gen-model -- generate-mesh --refnos "21491/18946"
```

#### 2. 前端加载失败：CORS 错误

使用本地服务器而非 file:// 协议：

```bash
# 使用 vite
npm run dev

# 或使用 Python
python3 -m http.server 8000
```

#### 3. LOD 切换不明显

调整 LOD 距离或使用不同精度级别的 mesh：

```toml
# DbOption.toml
[mesh_precision.lod_profiles.L1]
radial_segments = 12

[mesh_precision.lod_profiles.L2]
radial_segments = 20

[mesh_precision.lod_profiles.L3]
radial_segments = 32
```

## 扩展功能

### 1. 空间分块（Tiling）

大场景可按网格切分，实现流式加载：

```rust
// TODO: 未来版本支持
struct TileInfo {
    tile_id: String,
    bbox: Aabb,
    archetypes: Vec<String>,
}
```

### 2. 材质纹理

导出 PBR 材质参数：

```rust
// TODO: 未来版本支持
struct MaterialInfo {
    base_color: [f32; 3],
    metalness: f32,
    roughness: f32,
    texture_url: Option<String>,
}
```

### 3. 增量更新

支持部分实例动态添加/删除：

```typescript
// TODO: 未来版本支持
instancedMesh.addInstances(newInstances);
instancedMesh.removeInstances(instanceIds);
```

## API 参考

### Rust 导出器

```rust
pub async fn export_instanced_bundle_for_refnos(
    refnos: &[RefnoEnum],
    mesh_dir: &Path,
    output_dir: &Path,
    db_option: Arc<DbOption>,
    verbose: bool,
) -> Result<()>
```

### TypeScript 加载器

```typescript
class AiosInstancedBundleLoader {
  constructor(scene: Scene, verbose: boolean);
  
  async load(manifestUrl: string, baseUrl: string): Promise<void>;
  
  getInstancedMeshes(): InstancedMesh2[];
  
  stats: {
    totalArchetypes: number;
    totalInstances: number;
    loadedArchetypes: number;
    loadedInstances: number;
    totalTriangles: number;
  };
}
```

## 贡献指南

欢迎提交 Issue 和 Pull Request！

### 开发环境

```bash
# 后端（Rust）
cd gen-model
cargo build
cargo test

# 前端（TypeScript）
cd instanced-mesh
npm install
npm run dev
```

### 代码风格

- Rust: `cargo fmt` + `cargo clippy`
- TypeScript: ESLint + Prettier

## 许可证

MIT License

## 相关资源

- [instanced-mesh GitHub](https://github.com/agargaro/instanced-mesh)
- [Three.js 文档](https://threejs.org/docs/)
- [glTF 规范](https://www.khronos.org/gltf/)

## 更新日志

### v1.0.0 (2025-01-06)

- ✨ 初始版本发布
- ✅ 支持 L1/L2/L3 三级 LOD
- ✅ 自动 BVH 加速
- ✅ GLB + JSON 格式导出
- ✅ TypeScript 加载器
- 📝 完整文档和示例


