# PDMS 属性到代码映射关系调查报告

**调查日期**: 2025-12-10
**调查范围**: gen-model 项目中 PDMS 属性定义、读取、查询、网格生成的完整映射链路
**调查深度**: 跨模块代码追踪

---

## 代码部分（Evidence）

### 1. 属性定义与数据结构

#### 1.1 几何参数结构体 - `src/fast_model/pdms_inst.rs`
- **位置**: Line 79-87
- **类型**: `PdmsGeoParam::PrimSCylinder` (及其他几何体类型)
- **包含字段**:
  - `pdia: f64` - 直径（对应 ATT_DIAM）
  - `phei: f64` - 高度（对应 ATT_HEIG）
  - `btm_shear_angles: [f64; 2]` - 底部倾斜角度 [X, Y]
    - `btm_shear_angles[0]` - 底部X倾斜（对应 ATT_XBSH）
    - `btm_shear_angles[1]` - 底部Y倾斜（对应 ATT_YBSH）
  - `top_shear_angles: [f64; 2]` - 顶部倾斜角度 [X, Y]
    - `top_shear_angles[0]` - 顶部X倾斜（对应 ATT_XTSH）
    - `top_shear_angles[1]` - 顶部Y倾斜（对应 ATT_YTSH）
  - `unit_flag: bool` - 单位标志
  - `is_sscl()` - 方法，判断是否为特殊球面圆柱体

#### 1.2 属性映射汇总表

| PDMS 属性代码 | 含义 | gen-model 代码位置 | 数据类型 | 备注 |
|-------------|------|-----------------|--------|------|
| ATT_DIAM | 直径 | `PdmsGeoParam::PrimSCylinder.pdia` | f64 | 在 pdms_inst.rs:80 提取 |
| ATT_HEIG | 高度 | `PdmsGeoParam::PrimSCylinder.phei` | f64 | 在 pdms_inst.rs:81 提取 |
| ATT_XTSH | 顶部X倾斜 | `PdmsGeoParam::PrimSCylinder.top_shear_angles[0]` | f64 | 在 pdms_inst.rs:85 提取 |
| ATT_YTSH | 顶部Y倾斜 | `PdmsGeoParam::PrimSCylinder.top_shear_angles[1]` | f64 | 在 pdms_inst.rs:86 提取 |
| ATT_XBSH | 底部X倾斜 | `PdmsGeoParam::PrimSCylinder.btm_shear_angles[0]` | f64 | 在 pdms_inst.rs:83 提取 |
| ATT_YBSH | 底部Y倾斜 | `PdmsGeoParam::PrimSCylinder.btm_shear_angles[1]` | f64 | 在 pdms_inst.rs:84 提取 |

#### 1.3 其他支持的几何参数类型

`PdmsGeoParam` 枚举在 `pdms_inst.rs` 包含以下类型（Line 91-107）：
- `PrimBox` - 长方体
- `PrimLSnout` - 长锥形台
- `PrimDish` - 碟形
- `PrimSphere` - 球体
- `PrimCTorus` - 圆锥圆环
- `PrimRTorus` - 圆形圆环
- `PrimPyramid` - 金字塔
- `PrimLPyramid` - 长金字塔
- `PrimSCylinder` - 倾斜圆柱体（有倾斜端面）
- `PrimLCylinder` - 长圆柱体
- `PrimRevolution` - 旋转体
- `PrimExtrusion` - 拉伸体
- `PrimPolyhedron` - 多面体
- `PrimLoft` - 放样体
- `CompoundShape` - 复合形状
- `Unknown` - 未知类型

---

## 数据查询与读取流程

### 2. 属性查询接口

#### 2.1 主查询入口 - `src/fast_model/query.rs`
- **函数**: `query_gm_params(refno: RefnoEnum)` (Line 14-48)
- **职责**: 查询单个设计元素的所有几何体参数
- **工作流程**:
  1. 调用 `aios_core::collect_descendant_full_attrs()` 一次性查询所有子孙几何节点（深度1-2层）
  2. 过滤不可见几何体（Line 38-40）
  3. 对每个几何体调用 `query_gm_param()` 进行参数解析
  4. 返回 `Vec<GmParam>` - 所有几何参数

#### 2.2 底层参数解析 - aios_core 库
- **函数**: `aios_core::expression::query_cata::query_gm_param()`
  - **输入**: `NamedAttrMap` 属性映射、`is_spro` 标志
  - **处理**: 根据几何体类型解析属性并创建 `PdmsGeoParam`
  - **输出**: `GmParam` 结构体（包含 id、param、trans等）

#### 2.3 属性值读取 - `src/api/attr.rs`
- **核心类型**: `AttrMap` 和 `NamedAttrMap` (从 aios_core 导入)
- **转换函数**: `convert_row_to_attmap()` (Line 103+)
  - 将数据库行记录转换为 `AttrMap`
  - 处理属性哈希值映射

#### 2.4 几何参数提取方法
在 `prim_model.rs` (Line 141) 中：
```rust
let attr = aios_core::get_named_attmap(refno).await.unwrap_or_default();
```
然后调用 `attr.create_csg_shape()` 方法：
- **位置**: Line 281 (prim_model.rs)
- **职责**: 从属性映射创建 CSG 形状
- **处理**:
  - 提取属性值
  - 进行单位转换
  - 规范化倾斜角度
  - 创建可用于网格生成的形状对象

---

## 网格生成与几何参数使用

### 3. CSG 网格生成流程

#### 3.1 主网格生成入口 - `src/fast_model/mesh_generate.rs`
- **函数**: `gen_inst_meshes()` (Line 70-116)
- **调用链**:
  1. 从数据库查询 `GeoParam` 几何参数
  2. 对每个参数调用 `generate_csg_mesh()`
  3. 处理生成结果并保存

#### 3.2 参数查询与使用 - `src/fast_model/mesh_generate.rs`
- **位置**: Line 800-868（几何参数查询与CSG网格生成）
- **关键操作**:
  - Line 813-828: 提取 `PrimSCylinder` 参数用于调试日志
    - 提取：pdia, phei, btm_shear_angles[0], btm_shear_angles[1], top_shear_angles[0], top_shear_angles[1]
  - Line 849-851: 获取精度配置文件
  - Line 863-868: 调用 `generate_csg_mesh()`

#### 3.3 CSG 网格生成 - aios_core 库
- **函数**: `aios_core::geometry::csg::generate_csg_mesh()`
  - **输入参数**:
    - `&g.param: PdmsGeoParam` - 几何参数（包含所有PDMS属性）
    - `&profile.csg_settings: CsgSettings` - CSG精度设置
    - `non_scalable_geo: bool` - 是否为不可缩放几何体
    - `refno_for_mesh: Option<RefU64>` - 参考号（用于追踪）
  - **处理过程**:
    1. 对于 `PrimSCylinder`：
       - 使用 pdia 和 phei 定义基础圆柱体尺寸
       - 使用 btm_shear_angles 和 top_shear_angles 计算端面倾斜
       - 规范化倾斜角度到 [-90, 90] 范围
       - 计算端面法向量和中心点
       - 生成端面圆盘和侧面三角形网格
    2. 对其他几何体类型进行相应处理
  - **输出**: `GeneratedMesh` 对象

#### 3.4 几何参数数据库存储 - `src/fast_model/pdms_inst.rs`
- **函数**: `save_instance_data_optimize()` (Line 28-150)
- **存储位置**: SurrealDB `inst_geo` 表
- **保存的参数** (Line 71-150):
  - `geo_hash` - 几何哈希值
  - 完整的 `PdmsGeoParam` 结构体
  - 变换矩阵 (`transform`)
  - 关键点 (`key_points`)
  - 单位标志

---

## 变换应用流程

### 4. 局部变换与世界变换

#### 4.1 变换获取 - `src/fast_model/prim_model.rs`
- **位置**: Line 115
- **代码**:
```rust
let trans_result = aios_core::get_world_transform(refno).await;
let Ok(Some(mut trans_origin)) = trans_result else { continue; };
```
- **含义**: 获取参考号的世界坐标变换矩阵

#### 4.2 局部变换应用 - `src/fast_model/mesh_generate.rs`
- **位置**: Line 1272-1277
- **变换公式**:
```rust
let t = r.world_trans * g.trans;  // 世界变换 × 局部变换
let tmp_aabb = g.aabb.scaled(&t.scale.into());
let tmp_aabb = tmp_aabb.transform_by(&Isometry {
    rotation: t.rotation.into(),
    translation: t.translation.into(),
});
```
- **含义**:
  - `r.world_trans` - 所属对象的世界变换
  - `g.trans` - 几何体的局部变换
  - 最终变换：复合变换矩阵
  - AABB 应用旋转和平移

#### 4.3 关键点计算 - `src/fast_model/pdms_inst.rs`
- **位置**: Line 142-150
- **代码**:
```rust
let key_pts = inst.geo_param.key_points();
let mut pt_hashes = Vec::with_capacity(key_pts.len());
for key_pt in key_pts {
    let pts_hash = key_pt.gen_hash();
    pt_hashes.push(format!("vec3:⟨{}⟩", pts_hash));
    if let Entry::Vacant(entry) = vec3_map.entry(pts_hash) {
        entry.insert(serde_json::to_string(&key_pt)?);
    }
}
```
- **职责**: 提取几何体的关键点并存储到数据库

---

## 布尔运算中的属性使用

### 5. 布尔运算处理

#### 5.1 元件库级布尔运算 - `src/fast_model/manifold_bool.rs`
- **函数**: `apply_cata_neg_boolean_manifold()` (Line 156)
- **用途**: 处理有负体的元件库实例
- **流程**:
  1. 查询有负体标记的实例 (has_cata_neg = true)
  2. 获取正体网格和负体几何信息
  3. 执行 Manifold 库布尔差运算
  4. 更新 booled 标记

#### 5.2 几何参数在布尔运算中的角色
- 负体生成时需要完整的几何参数（包括所有倾斜属性）
- 布尔运算基于生成的三角形网格进行
- 属性值影响网格精度，进而影响布尔运算结果

---

## 调试与日志追踪

### 6. 调试信息记录

#### 6.1 参数日志 - `src/fast_model/pdms_inst.rs`
- **位置**: Line 74-123
- **记录内容** (JSON格式):
  - `geo_hash` - 几何哈希
  - `refno` - 参考号
  - `geo_type` - 几何体类型
  - `unit_flag` - 单位标志
  - `pdia` - 直径
  - `phei` - 高度
  - `btm[0], btm[1]` - 底部倾斜角度
  - `top[0], top[1]` - 顶部倾斜角度
  - `timestamp` - 时间戳

#### 6.2 网格生成日志 - `src/fast_model/mesh_generate.rs`
- **位置**: Line 808-846
- **记录内容** (JSON格式):
  - `chunk_idx` - 批处理索引
  - `geo_id` - 几何体ID
  - `geo_type` - 几何体类型
  - `pdia, phei` - 尺寸参数
  - `btm[], top[]` - 倾斜角度
  - `unit_flag` - 单位标志
  - `is_sscl` - 特殊球面圆柱体标志

---

## 关键流程总结

### 7. PDMS 属性到 3D 网格的完整映射链路

```
┌─────────────────────────────────────────┐
│  PDMS 数据库属性（ATT_DIAM 等）          │
└──────────────┬──────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────┐
│  AttrMap / NamedAttrMap 属性映射          │
│  (src/api/attr.rs)                      │
└──────────────┬──────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────┐
│  attr.create_csg_shape()                 │
│  (aios_core::expression::query_cata)    │
└──────────────┬──────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────┐
│  PdmsGeoParam::PrimSCylinder             │
│  - pdia (ATT_DIAM)                       │
│  - phei (ATT_HEIG)                       │
│  - btm_shear_angles[0] (ATT_XBSH)       │
│  - btm_shear_angles[1] (ATT_YBSH)       │
│  - top_shear_angles[0] (ATT_XTSH)       │
│  - top_shear_angles[1] (ATT_YTSH)       │
│  (src/fast_model/pdms_inst.rs)          │
└──────────────┬──────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────┐
│  query_gm_param() → GmParam              │
│  (aios_core::expression::query_cata)    │
└──────────────┬──────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────┐
│  generate_csg_mesh(param, settings)      │
│  (aios_core::geometry::csg)             │
│  - 规范化倾斜角度 [-90, 90]°            │
│  - 计算端面法向量                       │
│  - 生成三角形网格                       │
└──────────────┬──────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────┐
│  GeneratedMesh 对象                      │
│  + 变换应用 (world_trans × local_trans) │
└──────────────┬──────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────┐
│  布尔运算处理 (负体处理)                 │
│  (apply_cata_neg_boolean_manifold)      │
└──────────────┬──────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────┐
│  PLantMesh (最终网格)                    │
│  + AABB 缓存更新                         │
│  + 数据库保存                           │
└─────────────────────────────────────────┘
```

---

## 关键代码文件速查表

| 文件 | 关键行 | 职责 |
|-----|-------|------|
| `src/fast_model/pdms_inst.rs` | 79-107 | 几何参数结构体定义、参数提取 |
| `src/fast_model/query.rs` | 14-48 | 几何参数主查询入口 |
| `src/api/attr.rs` | 31-100 | 属性映射读取和转换 |
| `src/fast_model/prim_model.rs` | 100-299 | 基本体处理、CSG形状创建 |
| `src/fast_model/mesh_generate.rs` | 70-116, 800-900 | 网格生成主流程 |
| `src/fast_model/manifold_bool.rs` | 156-422 | 布尔运算处理 |
| aios_core 库 | - | 属性解析、网格生成、几何计算 |

---

## 验证建议

### 8. 属性映射验证清单

1. **属性提取验证**
   - 验证查询时是否正确提取了所有6个倾斜角度属性
   - 在调试日志中确认参数值的完整性

2. **角度规范化验证**
   - 验证倾斜角度是否规范化到 [-90, 90] 范围
   - 检查特殊情况处理（0°、90°、-90°）

3. **变换验证**
   - 验证局部变换和世界变换的复合是否正确
   - 检查倾斜端面的法向量在变换后是否正确

4. **布尔运算验证**
   - 使用倾斜圆柱体作为负体进行布尔运算
   - 验证结果网格的端面完整性和体积计算

---

## 相关文档参考

- `/llmdoc/architecture/mesh-generation-flow.md` - 网格生成流程详细文档
- `/llmdoc/architecture/fast-model-architecture.md` - Fast Model 架构设计
- `/llmdoc/guides/model-generation-guide.md` - 模型生成使用指南

---

**报告完成日期**: 2025-12-10
**调查深度**: 跨模块代码追踪 + 文档交叉验证
**置信度**: 90%（属性定义和查询链路明确，具体几何计算细节在 aios_core 库内部）
