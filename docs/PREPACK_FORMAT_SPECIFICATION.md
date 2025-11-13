# AIOS Instanced Prepack 格式规范

## 📋 概述

**Prepack** 是 AIOS 模型导出系统的一种优化格式，专为大规模工业模型的 Web 端高性能渲染设计。

### 核心特性

- **几何体共享**：所有几何体合并到单个 GLB 文件，通过索引引用
- **实例化渲染**：使用 InstancedMesh2 技术，支持百万级实例
- **数据压缩**：颜色调色板、名称表、索引引用等优化技术
- **按需加载**：支持分批加载、LOD 切换
- **层级结构**：保留 SITE → Component 层级关系

### 文件结构

```
output/
├── manifest.json              # 总清单
├── geometry_manifest.json     # 几何体清单
├── instances.json             # 实例数据
├── geometry_L1.glb            # LOD1（高精度）
├── geometry_L2.glb            # LOD2（中精度）
└── geometry_L3.glb            # LOD3（低精度）
```

---

## 📄 文件格式详解

### 1. manifest.json

**主清单文件**，描述整个 bundle 的元数据和文件引用。

#### 数据结构

```typescript
interface InstancedBundleManifest {
  version: string;                     // 语义化版本：主版本.次版本.补丁
  generated_at: string;                // 生成时间（ISO 8601）
  files: {
    geometry_manifest: FileRef;        // 几何清单文件
    instance_manifest: FileRef;        // 实例清单文件
    geometry_assets: Record<string, FileRef>; // LOD → GLB 文件
  };
  unit_conversion: UnitConversion;     // 结构化单位换算配置
  lod_profiles: LodProfile[];          // L1/L2/L3 等级配置
  stats: BundleStats;                  // 统计指标
}

interface FileRef {
  path: string;                        // 相对路径
  bytes: number;                       // 文件大小（字节）
  sha256: string;                      // 哈希值（校验完整性）
}

interface UnitConversion {
  source_unit: 'mm' | 'cm' | 'm' | string;
  target_unit: 'dm' | 'm' | string;
  factor: number;                      // 源 → 目标的缩放因子
  precision: number;                   // 小数精度（可选）
}

interface LodProfile {
  level: number;                       // 1=最高精度，数值越大越低
  target_triangles: number;            // 建议三角形数量上限
  max_position_error: number;          // 允许的顶点误差（目标单位）
  default_material: string;            // 缺省材质或着色器配置
  asset_key: string;                   // 对应 files.geometry_assets 的键名
  priority: number;                    // 加载优先级（值越小越先加载）
}

interface BundleStats {
  refno_count: number;                 // 唯一参考号数
  descendant_count: number;            // SITE → Component 的节点数
  unique_geometries: number;           // 几何体去重后的数量
  component_instances: number;         // 构件实例
  tubing_instances: number;            // 管道实例
  total_instances: number;             // component + tubing
  export_duration_ms: number;          // 导出耗时
}
```

#### 示例

```json
{
  "version": "1.1.0",
  "generated_at": "2025-11-12T10:30:45.123Z",
  "files": {
    "geometry_manifest": {
      "path": "geometry_manifest.json",
      "bytes": 1289456,
      "sha256": "b5c7..."
    },
    "instance_manifest": {
      "path": "instances.json",
      "bytes": 2456789,
      "sha256": "0b08..."
    },
    "geometry_assets": {
      "L1": {
        "path": "geometry_L1.glb",
        "bytes": 15500000,
        "sha256": "2c94..."
      },
      "L2": {
        "path": "geometry_L2.glb",
        "bytes": 7400000,
        "sha256": "5c68..."
      },
      "L3": {
        "path": "geometry_L3.glb",
        "bytes": 3500000,
        "sha256": "8a12..."
      }
    }
  },
  "unit_conversion": {
    "source_unit": "mm",
    "target_unit": "dm",
    "factor": 0.01,
    "precision": 6
  },
  "lod_profiles": [
    { "level": 1, "target_triangles": 200000, "max_position_error": 0.5, "default_material": "pbrStandard", "asset_key": "L1", "priority": 2 },
    { "level": 2, "target_triangles": 80000, "max_position_error": 2.5, "default_material": "litLambert", "asset_key": "L2", "priority": 1 },
    { "level": 3, "target_triangles": 20000, "max_position_error": 10.0, "default_material": "flatColor", "asset_key": "L3", "priority": 0 }
  ],
  "stats": {
    "refno_count": 1250,
    "descendant_count": 5640,
    "unique_geometries": 342,
    "component_instances": 5200,
    "tubing_instances": 440,
    "total_instances": 5640,
    "export_duration_ms": 12500
  }
}
```

---

### 2. geometry_manifest.json

**几何体清单**，描述所有几何体的元数据和在 GLB 中的位置。

#### 数据结构

```typescript
interface GeometryManifest {
  version: number;                     // 清单版本
  generated_at: string;                // 生成时间
  coordinate_system: {
    handedness: 'right';               // 当前仅支持右手坐标
    up_axis: 'Y' | 'Z';                // 默认 Y 轴向上
  };
  geometries: GeometryEntry[];         // 几何体列表
}

interface GeometryEntry {
  geo_hash: string;                    // 几何体哈希（唯一标识）
  geo_index: number;                   // 稠密索引（与 instances.json 对齐）
  nouns: string[];                     // 使用此几何体的构件类型列表
  vertex_count: number;                // 顶点数量（最高 LOD）
  triangle_count: number;              // 三角形数量（最高 LOD）
  bounding_box: BoundingBox | null;    // 局部包围盒
  bounding_sphere: BoundingSphere | null;
  lods: LodEntry[];                    // 每个 LOD 对应的 GLB 位置
}

interface BoundingBox {
  min: [number, number, number];
  max: [number, number, number];
}

interface BoundingSphere {
  center: [number, number, number];
  radius: number;
}

interface LodEntry {
  level: number;                       // 与 manifest.lod_profiles 对齐
  asset_key: string;                   // 指向 manifest.files.geometry_assets 的 key
  mesh_index: number;                  // GLB 中 mesh 索引
  node_index: number;                  // GLB 中 node 索引
  byte_range?: [number, number];       // 仅在二进制分块时填写
  triangle_count: number;
  error_metric: number;                // QEM 或 Hausdorff 误差
  material_override?: string;          // 特定 LOD 的材质配置
}
```

#### 示例

```json
{
  "version": 1,
  "generated_at": "2025-11-12T10:30:45.123Z",
  "coordinate_system": {
    "handedness": "right",
    "up_axis": "Y"
  },
  "geometries": [
    {
      "geo_hash": "11241886675982321911",
      "geo_index": 0,
      "nouns": ["ELBO", "REDU"],
      "vertex_count": 1248,
      "triangle_count": 832,
      "bounding_box": {
        "min": [-50.0, -50.0, -100.0],
        "max": [50.0, 50.0, 100.0]
      },
      "bounding_sphere": {
        "center": [0.0, 0.0, 0.0],
        "radius": 111.8
      },
      "lods": [
        {
          "level": 1,
          "asset_key": "L1",
          "mesh_index": 0,
          "node_index": 0,
          "triangle_count": 832,
          "error_metric": 0.0
        },
        {
          "level": 2,
          "asset_key": "L2",
          "mesh_index": 0,
          "node_index": 0,
          "triangle_count": 420,
          "error_metric": 1.2
        },
        {
          "level": 3,
          "asset_key": "L3",
          "mesh_index": 0,
          "node_index": 0,
          "triangle_count": 120,
          "error_metric": 6.5
        }
      ]
    }
  ]
}
```

---

### 3. instances.json

**实例清单**，包含所有实例的变换矩阵、颜色、名称等数据。

> **关于 geo_index**：`geo_index` 是 `geometry_manifest.geometries[].geo_index` 的稠密整数编号。导出阶段会保证 `(geo_hash, geo_index)` 一一对应，前端可以用 `geo_index` 常量时间定位几何体或执行 LOD 替换，而无需在渲染过程中对字符串 `geo_hash` 做 Map 查找。

#### 数据结构

```typescript
interface InstanceManifest {
  version: number;                     // 清单版本
  generated_at: string;                // 生成时间
  colors: [number, number, number, number][]; // RGBA 调色板
  names: NameEntry[];                  // 带类型的名称表
  components: ComponentGroup[];        // 构件实例列表
  tubings: TubingGroup[];              // 管道实例列表
}

interface NameEntry {
  kind: 'site' | 'zone' | 'component' | 'pipe' | string;
  value: string;
}

interface ComponentGroup {
  refno: string;                       // 构件参考号
  noun: string;                        // 构件类型（ELBO, PIPE 等）
  name_index: number | null;           // 在 names 表中的位置
  instances: InstanceEntry[];          // 实例列表
}

interface TubingGroup {
  refno: string;                       // 管道参考号
  noun: string;                        // 管道类型
  name_index: number | null;
  instances: InstanceEntry[];
}

interface InstanceEntry {
  geo_hash: string;                    // 几何体哈希（调试用）
  geo_index: number;                   // 稠密索引，对齐 geometry_manifest.geometries[].geo_index
  matrix: [number, number, number, number,
           number, number, number, number,
           number, number, number, number,
           number, number, number, number]; // 4×4 列主矩阵
  color_index: number;                 // 颜色索引
  name_index: number | null;           // 构件名称索引
  site_name_index: number;             // SITE 名称索引
  zone_name_index: number | null;      // Zone 名称索引
  lod_mask: number;                    // 位掩码，声明允许的 LOD（如 0b111 表示 L1/L2/L3）
  uniforms: Record<string, UniformValue>; // 扩展属性
}

type UniformValue = string | number | boolean | null;
```

#### 示例

```json
{
  "version": 1,
  "generated_at": "2025-11-12T10:30:45.123Z",
  "colors": [
    [0.82, 0.83, 0.84, 1.0],
    [0.75, 0.75, 0.75, 1.0],
    [0.90, 0.90, 0.90, 1.0]
  ],
  "names": [
    { "kind": "site", "value": "SITE_5107" },
    { "kind": "zone", "value": "Zone_21491" },
    { "kind": "component", "value": "ELBO_21491_10001" },
    { "kind": "component", "value": "PIPE_21491_10002" }
  ],
  "components": [
    {
      "refno": "21491_10001",
      "noun": "ELBO",
      "name_index": 2,
      "instances": [
        {
          "geo_hash": "11241886675982321911",
          "geo_index": 0,
          "matrix": [
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            100.5, 200.3, 50.8, 1.0
          ],
          "color_index": 0,
          "name_index": 2,
          "site_name_index": 0,
          "zone_name_index": 1,
          "lod_mask": 7,
          "uniforms": {
            "refno": "21491_10001"
          }
        }
      ]
    }
  ],
  "tubings": []
}
```

---

### 4. 几何体资产（geometry_L*.glb）

**多 LOD 几何体库**，每个 `LOD` 对应一个或多个 GLB 文件，并通过 `manifest.files.geometry_assets` 记录校验信息。

#### 特性

- **多文件**：常见配置为 `geometry_L1.glb / geometry_L2.glb / geometry_L3.glb`，也可根据 `asset_key` 拆分更多档位或按 Zone 切分。
- **索引访问**：`geometry_manifest.geometries[].lods[].mesh_index` 总是指向对应 GLB 内的 mesh 节点，前端通过 `(asset_key, mesh_index)` 精确定位需要的网格。
- **一致坐标系**：不同 LOD 的网格必须共享同一局部坐标原点和比例，保证实例在升级 LOD 时不会跳动。
- **二进制分块**：可选的 `byte_range` 字段允许将单个 GLB 当作字节流分块请求，实现更细粒度的流式加载。

#### 文件示例

| asset_key | 文件名          | 说明                   |
|-----------|-----------------|------------------------|
| `L1`      | `geometry_L1.glb` | 最高精度，发布时默认禁用增量压缩 |
| `L2`      | `geometry_L2.glb` | 中等精度，首屏优先加载            |
| `L3`      | `geometry_L3.glb` | 低精度兜底，保证快速成图          |

当 `LodEntry.level = 2` 且 `asset_key = "L2"` 时，渲染器需要：

1. 在 manifest 中找到 `files.geometry_assets.L2.path`（例如 `geometry_L2.glb`）。
2. 读取对应 GLB；若提供 `byte_range`，可只下载该范围。
3. 使用 `mesh_index` 和 `node_index` 在 GLB 场景树中获取具体 mesh 数据。

#### 结构示意

```
geometry_L2.glb
├── Scene
│   ├── Node_0 (Mesh_0)  ← lod:2 mesh_index:0
│   ├── Node_1 (Mesh_1)  ← lod:2 mesh_index:1
│   ├── Node_2 (Mesh_2)  ← lod:2 mesh_index:2
│   └── ...

geometry_L3.glb
├── Scene
│   ├── Node_0 (Mesh_0)  ← lod:3 mesh_index:0
│   ├── Node_5 (Mesh_5)  ← lod:3 mesh_index:5
│   └── ...
```

> **注意**：`mesh_index` 仅在各自 GLB 内局部递增，不同 LOD 的同一个 `geo_hash` 可以复用 `mesh_index` 值，但必须通过 `asset_key` 区分，渲染器最终以 `(geo_index, level)` 定位唯一网格。

---

## 🔧 数据优化技术

### 1. 颜色调色板

**问题**：每个实例存储完整的 RGBA 颜色会占用大量空间。

**解决方案**：
- 提取所有唯一颜色到 `colors` 数组
- 实例只存储 `color_index`
- 前端通过索引查找颜色

**示例**：
```json
{
  "colors": [
    [0.82, 0.83, 0.84, 1.0],  // index 0
    [0.75, 0.75, 0.75, 1.0]   // index 1
  ],
  "components": [{
    "instances": [
      { "color_index": 0, ... },  // 使用颜色 0
      { "color_index": 1, ... }   // 使用颜色 1
    ]
  }]
}
```

### 2. 名称表

**问题**：重复的名称字符串占用大量空间。

**解决方案**：
- 提取所有唯一名称到 `names` 数组
- 实例只存储 `name_index` 和 `site_name_index`
- 支持多级名称（SITE、Zone、Component）

### 3. 几何体共享

**问题**：相同几何体重复存储会产生大量冗余，并且不同精度版本难以统一。

**解决方案**：
- 使用 `geo_hash` 唯一标识几何体，并为其分配稠密 `geo_index`
- 将不同 LOD 的网格写入 `geometry_L*.glb` 等资产文件，通过 `asset_key` + `mesh_index` 精确定位
- 实例通过 `(geo_index, lod_mask)` 表示自己可以被哪些 LOD 网格渲染，渲染器可在后台替换

### 4. 矩阵编码

**格式**：4x4 矩阵，列主序（Column-major），16 个浮点数

```
[m11, m21, m31, m41,   // 第 1 列（X 轴）
 m12, m22, m32, m42,   // 第 2 列（Y 轴）
 m13, m23, m33, m43,   // 第 3 列（Z 轴）
 m14, m24, m34, m44]   // 第 4 列（平移）
```

**注意**：Three.js 使用列主序，与 OpenGL 一致。

---

## 🎨 前端加载流程

### 加载步骤

```typescript
// 1. 加载 manifest.json
const manifest = await fetch('manifest.json').then(r => r.json());

// 2. 预加载最低优先级 LOD（如 L3）
const lodQueue = manifest.lod_profiles
  .slice()
  .sort((a, b) => a.priority - b.priority);

const geometryCaches = new Map();

async function loadLod(lod) {
  const asset = manifest.files.geometry_assets[lod.asset_key];
  const gltf = await gltfLoader.loadAsync(asset.path);
  geometryCaches.set(lod.level, extractGeometriesFromGLTF(gltf.scene));
}

// 2a. 先加载优先级最低的低精度 LOD，保证快速成图
await loadLod(lodQueue[0]);

// 3. 加载 geometry_manifest.json
const geometryManifest = await fetch(manifest.geometry_manifest).then(r => r.json());

// 4. 加载 instances.json
const instanceManifest = await fetch(manifest.instance_manifest).then(r => r.json());

// 5. 按 geo_index 分组实例（更便于稠密数组）
const instancesByGeoIndex = groupInstancesByGeoIndex(instanceManifest);

// 6. 为每个 geo_index 创建 InstancedMesh2（先用低 LOD）
for (const [geoIndex, instances] of instancesByGeoIndex) {
  const geometryEntry = geometryManifest.geometries[geoIndex];
  const lodLevel = pickInitialLod(instances); // 例如根据 lod_mask 或全局默认
  const mesh = geometryCaches.get(lodLevel).get(
    geometryEntry.lods.find(l => l.level === lodLevel).mesh_index
  );

  const instancedMesh = new InstancedMesh2(mesh, material, instances.length);
  instances.forEach((inst, i) => {
    instancedMesh.setMatrixAt(i, new Matrix4().fromArray(inst.matrix));
    instancedMesh.setColorAt(i, new Color().fromArray(instanceManifest.colors[inst.color_index]));
  });
  scene.add(instancedMesh);
}

// 7. 在空闲帧逐步加载更高 LOD 并替换
requestIdleCallback(async () => {
  for (const lod of lodQueue.slice(1)) {
    await loadLod(lod);
    upgradeVisibleMeshes(lod.level);
  }
});
```

### 参考实现

完整的前端加载器实现请参考：
- `instanced-mesh/examples/aios-prepack-loader.ts`
- `instanced-mesh/examples/aios-prepack-loader.html`

---

## 📐 线框图（多 LOD 按优先级加载）

```
+------------------------+      +-----------------------+      +----------------------+
| manifest.json          |      | geometry_manifest.json|      | instances.json       |
| - files + hashes       |      | - geo_index / LOD map |      | - geo_index / matrices|
+-----------+------------+      +-----------+-----------+      +-----------+----------+
            |                               |                               |
            v                               v                               v
  +--------------------+        +----------------------+          +--------------------+
  | LOD Scheduler      |        | Geometry Cache       |          | Instance Grouper   |
  | - sort by priority |        | - per LOD mesh pool  |          | - group by geo_idx |
  +------+-------------+        +-----------+----------+          +-----------+--------+
         |                                |                                 |
         v                                v                                 v
  +--------------------+        +----------------------+          +--------------------+
  | Streaming Loader   |------->| Low LOD ready?       |<---------| InstancedMesh Pool |
  | - fetch GLB chunks |        | -> render baseline   |          | - set matrices      |
  +--------+-----------+        +-----------+----------+          | - bind colors/Lights|
           |                                |                     +-----------+--------+
           v                                v                                 |
  +--------------------+        +----------------------+                     |
  | Idle-time Upgrader |<-------| Higher LOD available |<---------------------+
  | - swap geometry    |        | - per-instance mask  | 事件驱动             |
  | - update progress  |        |                      | (lod_mask / visibility)
  +--------------------+        +----------------------+
```

该线框图展示了“先渲染低精度、再按优先级升级”的流程：`manifest.json` 提供文件校验与 LOD 配置；`geometry_manifest.json` 负责 `geo_index ↔ LOD` 映射；`instances.json` 通过 `geo_index`/`lod_mask` 将实例分组，确保 InstancedMesh 可直接引用正确的网格；后台 `Streaming Loader` 负责增量下载 GLB，`Idle-time Upgrader` 则在浏览器空闲帧内将已经显示的实例替换为更高精度。

---

## 📊 性能指标

### 典型场景（5107 数据库）

| 指标 | 数值 |
|------|------|
| 构件数量 | 5,640 |
| 唯一几何体 | 342 |
| 总实例数 | 5,640 |
| GLB 文件大小 | ~15 MB |
| instances.json 大小 | ~2.5 MB |
| 加载时间 | ~3-5 秒 |
| 渲染帧率 | 60 FPS |

### 优化效果

相比传统格式（每个几何体一个 GLB）：
- **文件数量**：从 342 个减少到 1 个（-99.7%）
- **总文件大小**：减少约 40-60%
- **加载时间**：减少约 70-80%
- **内存占用**：减少约 50-60%

---

## 🔄 版本历史

### v1.0（当前版本）

- 初始版本
- 支持颜色调色板、名称表
- 支持 SITE 层级结构
- 多 LOD 几何体资产（geometry_L*.glb）

### 未来计划

- **v1.1**：支持多 LOD 级别
- **v1.2**：支持流式加载
- **v1.3**：支持增量更新

---

## 📚 相关文档

- [导出器实现](../src/fast_model/export_model/export_prepack_lod.rs)
- [前端加载器](../../instanced-mesh/examples/aios-prepack-loader.ts)
- [InstancedMesh2 文档](../../instanced-mesh/README.md)

---

## 🤝 贡献指南

如需修改格式规范，请：
1. 更新本文档
2. 更新导出器实现
3. 更新前端加载器
4. 添加测试用例
5. 更新版本号

---

**最后更新**：2025-11-12  
**维护者**：AIOS Team
