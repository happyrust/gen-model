# 网格生成与布尔运算流程

## 概述
本文档描述从 PDMS 几何参数到最终网格文件的完整处理流程。

## 主要流程

```
┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│ 查询几何参数 │───▶│ 生成基础网格 │───▶│ 布尔运算处理 │
└──────────────┘    └──────────────┘    └──────────────┘
                                               │
┌──────────────┐    ┌──────────────┐           ▼
│ 更新 AABB    │◀───│ 保存网格文件 │◀───┬──────────────┐
└──────────────┘    └──────────────┘    │ 合并/去重    │
                                        └──────────────┘
```

## 1. 查询几何参数

### 数据来源
```rust
// 从 SurrealDB 查询几何参数
let geo_params: Vec<GeoParam> = query_geo_params(&refnos).await?;
let aabb_params: Vec<QueryAabbParam> = query_aabb_params(&refnos).await?;
```

### 几何参数类型
- **CATE**: 元件库引用 + 变换矩阵
- **PRIM**: 基本体参数 (Box/Cylinder/Cone/Sphere)
- **LOOP**: 轮廓 + 拉伸/旋转参数

## 2. 生成基础网格

### 入口函数
```rust
pub async fn gen_inst_meshes(
    refnos: &[RefnoEnum],
    replace_exist: bool,
    mesh_dir: String,
    precision: Arc<MeshPrecisionSettings>,
) -> anyhow::Result<()>
```

### 网格生成策略
| 类型 | 生成方式 |
|------|----------|
| CATE | 从元件库模板实例化 |
| PRIM | CSG 基本体生成 |
| LOOP | 轮廓拉伸/旋转 |

### CSG 网格生成
```rust
use aios_core::geometry::csg::generate_csg_mesh;

let mesh: GeneratedMesh = generate_csg_mesh(&geo_param, &precision)?;
```

## 3. 布尔运算处理

### Manifold 库集成
**文件**: `manifold_bool.rs`

```rust
// 元件库级布尔运算（处理负体）
pub async fn apply_cata_neg_boolean_manifold(
    refno: RefnoEnum,
    pos_mesh: &PlantMesh,
    neg_infos: &[NegInfo],
) -> anyhow::Result<PlantMesh>

// 实例级布尔运算
pub async fn apply_insts_boolean_manifold(
    refnos: &[RefnoEnum],
) -> anyhow::Result<Vec<(RefnoEnum, PlantMesh)>>
```

### 布尔运算类型
- **Union (并)**: 合并多个正体
- **Subtract (差)**: 从正体中减去负体
- **Intersect (交)**: 取交集部分

### 布尔运算流程
```
1. 查询 has_cata_neg = true 的实例
2. 获取负体几何信息
3. 执行 Manifold 布尔差运算
4. 更新 booled 标记
5. 保存结果网格
```

## 4. 网格文件保存

### 文件格式
- **位置**: `assets/meshes/{geo_hash}.bin`
- **格式**: 二进制序列化 (rkyv)
- **去重**: 基于几何哈希

### 哈希计算
```rust
let geo_hash = gen_bytes_hash(&mesh_bytes);
// 存储到 EXIST_MESH_GEO_HASHES 避免重复
```

## 5. AABB 更新

### 更新函数
```rust
pub async fn update_inst_relate_aabbs_by_refnos(
    refnos: &[RefnoEnum],
    replace_exist: bool,
) -> anyhow::Result<()>
```

### SQLite 空间索引
```rust
// 使用 SQLite R*-tree 进行空间查询优化
let spatial_index = SqliteSpatialIndex::open()?;
spatial_index.insert(&bbox)?;
```

## 性能优化

### 批量处理
```rust
// 每批 100 个 refno
for chunk in refnos.chunks(100) {
    gen_inst_meshes(chunk, ...).await?;
    update_inst_relate_aabbs_by_refnos(chunk, ...).await?;
}
```

### 并发控制
- 使用 `DashMap` 缓存已处理网格
- `EXIST_MESH_GEO_HASHES` 全局缓存避免重复生成

### 错误处理
```rust
// 记录处理失败的 refno
record_refno_error(refno, RefnoErrorKind::MeshGeneration, stage, msg);
```
