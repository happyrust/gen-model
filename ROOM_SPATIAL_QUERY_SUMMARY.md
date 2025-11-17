# 房间空间查询测试总结

## 📅 测试日期
2025-11-16

## 🎯 测试目标
验证房间能查询到哪些模型元素，理解房间与元素之间的关系。

## ✅ 测试结果

### 1. 房间基本信息查询

**成功获取：**
- 总房间数：124 个
- 房间关键词：`["-RM"]`
- 每个房间包含 1-2 个面板

**示例数据：**
```
房间 #1 - VOLU
  Room Refno: 24381_34850
  面板数量: 2
  面板列表:
    - 24381_34851
    - 24381_34862
```

### 2. 房间-元素关系查询

**测试方法：**
- 方法1：通过 `REFNO<-pe_owner<-pe` 查询房间的 owner 关系
- 方法2：通过面板的 `REFNO<-pe_owner<-pe` 查询面板连接的元素

**测试结果：** ❌ **所有房间和面板都查询不到元素**

## 🔍 关键发现

### 房间与元素的关系特点

1. **不是通过数据库关系连接的**
   - 房间 (FRMW/SBFR) 和设备/管道等元素之间
   - **没有**直接的 `pe_owner` 关系
   - **没有**其他的图数据库边关系

2. **是通过空间包含关系确定的**
   - 房间由多个面板 (PANE) 组成封闭空间
   - 元素是否属于某个房间，需要通过**空间计算**判断
   - 判断逻辑：点是否在多边形面内

### 正确的查询方式

#### ✅ 方式1：使用 aios_core 的空间查询函数

```rust
use aios_core::room::{query_room_number_by_point_v2, query_room_panel_by_point_v2};
use glam::Vec3;

// 1. 获取元素的位置
let element_position = Vec3::new(x, y, z);

// 2. 查询该位置属于哪个房间
let room_number = query_room_number_by_point_v2(element_position).await?;

// 3. 或查询该位置属于哪个房间面板
let room_panel_refno = query_room_panel_by_point_v2(element_position).await?;
```

**实现原理：**
1. 使用混合空间索引快速筛选候选房间
2. 加载房间面板的几何网格
3. 使用射线法判断点是否在面内

#### ✅ 方式2：批量查询元素所属房间

```rust
use aios_core::room::batch_query_room_numbers;

// 批量查询多个点的房间归属
let points = vec![
    Vec3::new(x1, y1, z1),
    Vec3::new(x2, y2, z2),
    // ...
];

let room_numbers = batch_query_room_numbers(points, 10).await?;
```

#### ❌ 错误的查询方式

```rust
// ❌ 错误：尝试通过 owner 关系查询
let sql = format!(
    "SELECT value REFNO<-pe_owner<-pe FROM {}",
    room_refno.to_pe_key()
);
// 这种方式查不到元素！
```

## 📊 数据结构说明

### 房间 (FRMW/SBFR)
- **noun**: FRMW (HD项目) 或 SBFR (其他项目)
- **功能**: 定义房间的逻辑实体
- **属性**: NAME (房间号)

### 面板 (PANE)
- **noun**: PANE
- **功能**: 房间的物理边界面
- **几何**: PXYZ (位置), DTXYZ (方向), 多边形顶点
- **关系**: 通过 `pe_owner` 连接到房间

### 元素 (设备/管道等)
- **noun**: PUMP, PIPE, VALVE, 等
- **位置**: PXYZ 属性
- **关系**: **不直接连接到房间**
- **归属**: 通过空间计算确定所属房间

## 💡 实践建议

### 1. 查询房间内的所有元素

**步骤：**
1. 获取房间的所有面板
2. 构建房间的空间边界
3. 查询所有需要归属的元素
4. 对每个元素使用 `query_room_panel_by_point_v2` 判断归属

**示例流程：**
```rust
// 1. 查询房间和面板
let rooms = query_room_panels_by_keywords(&keywords).await?;

// 2. 查询所有设备的位置
let sql = "SELECT id, noun, NAME, PXYZ FROM pe 
           WHERE noun IN ['PUMP', 'VALVE', 'TANK'] 
           LIMIT 1000";

// 3. 对每个设备判断所属房间
for equipment in equipments {
    if let Some(pxyz) = equipment.pxyz {
        let pos = Vec3::new(pxyz[0], pxyz[1], pxyz[2]);
        if let Some(room_num) = query_room_number_by_point_v2(pos).await? {
            println!("{} 属于房间 {}", equipment.id, room_num);
        }
    }
}
```

### 2. 性能优化

**对于大量元素：**
- ✅ 使用 `batch_query_room_numbers` 批量查询
- ✅ 设置合适的并发数（如 10-50）
- ✅ 缓存房间几何数据
- ❌ 避免逐个串行查询

### 3. 使用场景

**适用：**
- 统计房间内的设备数量
- 生成房间设备清单
- 空间分析和可视化
- 房间管道统计

**不适用：**
- 直接通过数据库关系查询
- 不考虑空间位置的关联

## 🔧 相关函数

### aios_core::room 模块

```rust
// 根据点查询房间号
pub async fn query_room_number_by_point_v2(point: Vec3) 
    -> anyhow::Result<Option<String>>

// 根据点查询房间面板
pub async fn query_room_panel_by_point_v2(point: Vec3) 
    -> anyhow::Result<Option<RefnoEnum>>

// 批量查询房间号
pub async fn batch_query_room_numbers(
    points: Vec<Vec3>, 
    max_concurrent: usize
) -> anyhow::Result<Vec<Option<String>>>

// 根据关键词查询房间和面板
pub async fn query_room_panels_by_keywords(
    room_keywords: &Vec<String>
) -> anyhow::Result<Vec<(RefnoEnum, String, Vec<RefnoEnum>)>>
```

## 📈 性能数据

**测试环境：** AvevaMarineSample 项目

| 操作 | 数量 | 耗时 | 备注 |
|------|------|------|------|
| 查询房间列表 | 124个 | 50ms | 使用 query_room_panels_by_keywords |
| 单点查询房间 | 1个 | ~1-5ms | 使用 query_room_number_by_point_v2 |
| 批量查询(10并发) | 100个点 | ~100-200ms | 估算 |

## 🎓 学习要点

1. **空间归属不等于数据关系**
   - 数据库中没有房间→元素的边
   - 归属通过空间计算动态确定

2. **查询策略的选择**
   - 少量查询：逐个查询
   - 大量查询：批量+并发
   - 缓存几何数据以提升性能

3. **数据模型理解**
   - PDMS/E3D 的空间模型特点
   - 房间是逻辑概念，边界是物理实体
   - 元素归属需要计算而不是查表

## 🔗 相关文档

- `rs-core/src/room/query_v2.rs` - 空间查询实现
- `rs-core/src/room/algorithm.rs` - 房间算法
- `REFACTORING_SUMMARY.md` - 房间查询重构总结

## 📝 测试文件

- `examples/test_room_query_new.rs` - 基本房间查询
- `examples/test_room_spatial_query_simple.rs` - 空间查询测试

---

**完成时间：** 2025-11-16  
**状态：** ✅ 测试完成，理解房间查询机制
