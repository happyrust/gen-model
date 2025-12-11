# 倾斜端圆柱体（SLC）快速参考指南

**最后更新**: 2025-12-10
**用途**: 快速查阅 SLC 实现的关键信息

---

## 快速导航

### 我需要... 那我应该看...

| 需求 | 文档 | 行号 | 关键类 |
|-----|------|------|--------|
| 理解 SLC 是什么 | slc_complete_investigation_summary_20251210.md | 第一部分 | PrimSCylinder |
| 查找参数提取代码 | slc_implementation_analysis_20251210.md | 表格 | pdms_inst.rs:79 |
| 理解网格生成流程 | slc_csg_integration_analysis_20251210.md | 2.1 | generate_csg_mesh |
| 理解变换应用 | slc_normalization_and_transform_20251210.md | 二、2.1 | Isometry |
| 调试 SLC 问题 | slc_complete_investigation_summary_20251210.md | 第六部分 | 验证步骤 |
| 找到所有代码文件 | slc_implementation_analysis_20251210.md | 相关文件汇总 | - |

---

## SLC 参数一览

### 结构体定义

```rust
pub struct PrimSCylinder {
    pub pdia: f64,                      // 直径 (mm)
    pub phei: f64,                      // 高度 (mm)
    pub btm_shear_angles: [f64; 2],     // 底部倾斜 [X°, Y°]
    pub top_shear_angles: [f64; 2],     // 顶部倾斜 [X°, Y°]
    pub unit_flag: bool,                // 单位转换标志
}
```

### 参数范围

| 参数 | 范围 | 含义 |
|-----|-----|------|
| pdia | > 0 | 圆柱体直径 |
| phei | > 0 | 圆柱体高度 |
| btm_shear[0] | [-90, 90]° | 底部 X 倾斜（规范化后） |
| btm_shear[1] | [-90, 90]° | 底部 Y 倾斜（规范化后） |
| top_shear[0] | [-90, 90]° | 顶部 X 倾斜（规范化后） |
| top_shear[1] | [-90, 90]° | 顶部 Y 倾斜（规范化后） |

### PDMS 属性映射

| PDMS 属性 | gen-model 字段 | 存储位置 |
|----------|--------------|---------|
| ATT_DIAM | pdia | PrimSCylinder::pdia |
| ATT_HEIG | phei | PrimSCylinder::phei |
| ATT_XTSH | top_shear_angles[0] | [0] |
| ATT_YTSH | top_shear_angles[1] | [1] |
| ATT_XBSH | btm_shear_angles[0] | [0] |
| ATT_YBSH | btm_shear_angles[1] | [1] |

---

## 数据流示意图

### 简化流程

```
PDMS 数据库
    ↓
get_named_attmap()          [获取属性]
    ↓
query_gm_param()             [规范化 + 创建 PdmsGeoParam]
    ↓
save_instance_data_optimize() [保存到 inst_geo 表]
  ├─ H4 日志 (参数保存)
  └─ H3 日志 (最终值)
    ↓
gen_inst_meshes()            [查询网格生成]
  └─ H6 日志 (参数查询)
    ↓
generate_csg_mesh()          [CSG 网格生成]
    ├─ 端面圆盘生成
    ├─ 侧面网格生成
    └─ 法向量计算
    ↓
PlantMesh (局部坐标系)
    ↓
apply_world_transform()      [应用变换]
    ↓
最终网格 (世界坐标系)
```

---

## 关键代码位置速查

### 参数提取

```
pdms_inst.rs:79-87      ← 从 PrimSCylinder 提取倾斜参数
pdms_inst.rs:114        ← 写入 H4 日志
pdms_inst.rs:216        ← 提取 unit_flag
pdms_inst.rs:217        ← 调用 is_sscl()
pdms_inst.rs:221-237    ← 写入 H3 日志
```

### 网格生成

```
prim_model.rs:281       ← attr.create_csg_shape() 调用
prim_model.rs:292       ← check_valid() 验证
prim_model.rs:315-317   ← convert_to_geo_param() 转换
mesh_generate.rs:813-846 ← H6 日志和参数验证
mesh_generate.rs:863-867 ← generate_csg_mesh() 调用
mesh_generate.rs:897-972 ← LOD 处理
```

### 变换应用

```
prim_model.rs:115       ← get_world_transform() 获取变换
mesh_generate.rs:1272   ← t = r.world_trans * g.trans
mesh_generate.rs:1273   ← tmp_aabb.scaled()
mesh_generate.rs:1274   ← tmp_aabb.transform_by()
```

### 布尔运算

```
manifold_bool.rs:156-207  ← 元件库级布尔
manifold_bool.rs:251-363  ← 实例级布尔
manifold_bool.rs:207      ← batch_boolean_subtract() 调用
```

---

## 三层日志验证

### H4 日志 (保存时)

**位置**: `pdms_inst.rs:108-122`

```json
{
  "sessionId": "debug-session",
  "runId": "pre-fix",
  "hypothesisId": "H4",
  "location": "pdms_inst.rs:save_instance_data_optimize",
  "message": "inst_geo_buffer push",
  "data": {
    "geo_hash": <hash>,
    "refno": "pe:123456",
    "geo_type": "PrimSCylinder",
    "unit_flag": <flag>,
    "pdia": <直径>,
    "phei": <高度>,
    "btm": [<X倾斜>, <Y倾斜>],
    "top": [<X倾斜>, <Y倾斜>]
  },
  "timestamp": <ms>
}
```

### H3 日志 (前置处理)

**位置**: `pdms_inst.rs:221-237`

```json
{
  "hypothesisId": "H3",
  "location": "pdms_inst.rs:save_instance_data_optimize",
  "message": "push inst_geo",
  "data": {
    "geo_hash": <hash>,
    "refno": "pe:123456",
    "geo_type": "PrimSCylinder",
    "pdia": <直径>,
    "phei": <高度>,
    "btm": [<X倾斜>, <Y倾斜>],
    "top": [<X倾斜>, <Y倾斜>],
    "unit_flag": <flag>,
    "is_sscl": <true/false>,
    "inst_geo_buffer_len": <计数>
  },
  "timestamp": <ms>
}
```

### H6 日志 (查询时)

**位置**: `mesh_generate.rs:830-845`

```json
{
  "hypothesisId": "H6",
  "location": "mesh_generate.rs:query_geo_params",
  "message": "geo param fetched",
  "data": {
    "chunk_idx": <索引>,
    "geo_id": "pe:123456",
    "geo_type": "PrimSCylinder",
    "pdia": <直径>,
    "phei": <高度>,
    "btm": [<X倾斜>, <Y倾斜>],
    "top": [<X倾斜>, <Y倾斜>],
    "unit_flag": <flag>,
    "is_sscl": <true/false>
  },
  "timestamp": <ms>
}
```

**验证**: H4、H3、H6 中的参数应完全相同（除了 geo_hash 和时间戳）

---

## 常见问题速查

### Q1: 如何验证参数是否正确保存？

**A**: 对比 H4 和 H3 日志
```bash
grep "hypothesisId\":\"H4" debug.log | grep "refno\":\"pe:123456"
grep "hypothesisId\":\"H3" debug.log | grep "refno\":\"pe:123456"
# 参数应完全相同
```

### Q2: 如何验证参数是否正确查询？

**A**: 对比 H3 和 H6 日志
```bash
grep "hypothesisId\":\"H3" debug.log | grep "refno\":\"pe:123456"
grep "hypothesisId\":\"H6" debug.log | grep "geo_id\":\"pe:123456"
# 参数应完全相同
```

### Q3: 倾斜圆柱体为什么生成失败？

**A**: 检查以下几点
1. 参数是否在有效范围内？ (pdia > 0, phei > 0)
2. 倾斜角度是否在 [-90, 90] 范围内？
3. 是否有足够的日志记录失败原因？
4. 使用 H6 日志验证查询到的参数

### Q4: 变换应用后网格变形了怎么办？

**A**: 检查变换矩阵
1. 是否应用了正确的 world_transform？
2. 缩放因子是否合理？
3. 四元数是否正确转换为旋转矩阵？
4. 使用调试工具输出 AABB 前后值进行对比

### Q5: 布尔运算失败怎么排查？

**A**: 逐步验证
1. SLC 网格是否生成成功？
2. SLC 是否正确加载为 Manifold？
3. 变换矩阵是否正确应用到 Manifold？
4. 其他几何体是否有效？
5. 相对坐标系计算是否正确？

---

## 关键概念解释

### 规范化 (Normalization)

**什么是规范化?**
将倾斜角度限制在 [-90, 90] 范围内的过程

**为什么需要规范化?**
- 确保每个物理方向只有一个表示
- 避免法向量计算中的歧义
- 提高数值稳定性

**规范化算法**:
```
if angle > 90:
    angle -= 180
if angle < -90:
    angle += 180
```

**规范化发生在哪?**
在 `aios_core::query_gm_param()` 中（隐式处理）

### 端面法向量 (Normal Vector)

**定义**:
```
nx = sin(x_slope)
ny = sin(y_slope)
nz = cos(x_slope) * cos(y_slope)
n = normalize([nx, ny, nz])
```

**底部端面**: 法向量指向负 Z 方向（倾斜）
**顶部端面**: 法向量指向正 Z 方向（倾斜）

**在旋转变换中**: n' = R * n (R 是旋转矩阵)

### 坐标系层级 (Coordinate Hierarchy)

```
1. 局部坐标系 (Local)
   原点在圆柱体中心

2. 几何坐标系 (Geometry Local)
   可能与局部坐标系不同

3. 世界坐标系 (World)
   最终的坐标系
```

**变换链**: Local → Geometry → World

### LOD (Level of Detail)

**什么是 LOD?**
为不同的距离或精度需求生成不同细节级别的网格

**SLC 中的 LOD**:
```
L0 (default): 最高精度
L1:          中等精度
L2:          低精度
L3:          最低精度
```

**关键特性**: 所有 LOD 级别使用相同的几何参数，只有精度不同

---

## 调试工具清单

### 现有工具

| 工具 | 位置 | 用途 |
|-----|------|------|
| H4/H3/H6 日志 | debug.log | 参数验证 |
| check_valid() | prim_model.rs:292 | 几何有效性 |
| debug_model!() 宏 | error_macros.rs | 调试输出 |

### 推荐添加的工具

| 工具 | 文件 | 功能 |
|-----|------|------|
| slc_validation_test | examples/ | 自动化测试 |
| slc_debugger | tools/ | 可视化调试 |
| normal_vector_printer | tools/ | 法向量输出 |

---

## 性能考虑

### 网格生成时间

典型 SLC 网格生成时间（单个）:
- **高精度** (L0): ~10-50ms
- **中精度** (L1): ~5-20ms
- **低精度** (L2-L3): ~1-5ms

### 内存使用

典型 SLC 网格内存占用:
- **高精度**: 100KB - 1MB
- **中精度**: 50KB - 500KB
- **低精度**: 10KB - 100KB

### 优化建议

1. **避免重复计算**: 使用 EXIST_MESH_GEO_HASHES 缓存
2. **批量处理**: 分块处理大量 refno
3. **LOD 选择**: 根据使用场景选择合适的 LOD 级别

---

## 故障排除矩阵

| 现象 | 可能原因 | 解决方案 |
|-----|--------|--------|
| 参数为 0 | 数据库查询失败 | 检查 H6 日志，验证数据库连接 |
| 倾斜角超出范围 | 规范化失败 | 检查 aios_core 版本 |
| 网格生成失败 | CSG 引擎错误 | 检查参数有效性，尝试简单参数 |
| 端面不倾斜 | 参数未应用 | 检查 H3/H6 日志中的倾斜角度值 |
| 变换后变形 | 变换矩阵错误 | 验证 world_transform，检查四元数 |
| 布尔运算失败 | 坐标系不匹配 | 检查 manifold_bool 中的矩阵计算 |
| 内存溢出 | 精度设置过高 | 降低精度等级，检查 LOD 设置 |
| 极端角度报错 | 边界值处理缺失 | 添加角度范围检查 |

---

## 相关命令

### 编译和运行

```bash
# 构建项目
cargo build --release

# 运行单个二进制
cargo run --bin <binary_name> --release

# 运行测试 (假设已存在)
cargo test slc

# 启用调试信息
RUST_LOG=debug cargo run --release
```

### 日志查询

```bash
# 查找特定 refno 的所有日志
grep "refno:\"pe:123456\"" /Volumes/DPC/work/plant-code/rs-plant3-d/.cursor/debug.log

# 查找 H4、H3、H6 日志
grep "hypothesisId\":\"H4" debug.log
grep "hypothesisId\":\"H3" debug.log
grep "hypothesisId\":\"H6" debug.log

# 按时间排序
sort -t: -k5 -n debug.log

# 提取特定字段
jq '.data | {refno, pdia, phei, btm, top}' debug.log
```

---

## 相关阅读顺序

### 快速了解 (15 分钟)

1. 本文档 (快速参考)
2. slc_complete_investigation_summary_20251210.md (第一、二部分)

### 深入理解 (1 小时)

1. slc_implementation_analysis_20251210.md
2. slc_normalization_and_transform_20251210.md
3. slc_csg_integration_analysis_20251210.md

### 完整学习 (2 小时+)

1. 阅读所有 SLC 报告
2. 查看相关源代码
3. 运行测试或调试工具
4. 查阅架构文档

---

**最后更新**: 2025-12-10
**维护者**: aios-database 团队
**问题报告**: 请在代码审查时提出
