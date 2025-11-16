<!-- 5f1b52ae-d6e1-4236-8c39-fa3cbc979ce2 923f4d96-e72e-45ce-9ee9-a8f0ee3aaba1 -->
# BRAN 房间相交计算实现方案

## 一、核心需求确认

1. **几何计算方式**：使用 BRAN 中心线（PipelineSpan）与房间 tri_mesh 进行精确相交计算
2. **主次房间阈值**：长度 >= 1.0 米才考虑，最长为主房间
3. **更新策略**：支持增量更新（BRAN 或房间变更时重新计算）
4. **性能要求**：支持批量处理 10 个 BRAN

## 二、技术方案

### 1. 数据结构设计

**核心数据结构**：

- `BranRoomIntersection`：BRAN 与房间的相交记录
- `IntersectionSegment`：相交线段详情
- `BranRoomCalculationResult`：计算结果汇总

**关键字段**：

- `total_length`：BRAN 在该房间内的总长度（米）
- `is_primary`：是否为主房间（长度最长且 >= 1.0 米）
- `intersection_segments`：所有相交线段列表
- `confidence`：计算置信度（基于几何精度）

### 2. 核心算法流程

#### 2.1 BRAN 中心线提取

- 使用 `PipelineQueryService::fetch_branch_segments(branch_refno)` 获取所有管段
- 提取每个 `PipelineSegmentRecord` 的 `main_span()` 和 `branch_spans()`
- 将每个 `PipelineSpan` 转换为线段：`(start.world_pos, end.world_pos)`
- 按流程顺序排序线段（保持 BRAN 的连续性）

#### 2.2 房间 tri_mesh 获取

- 使用空间索引查询与 BRAN AABB 相交的候选房间
- 对每个房间，获取其 `RoomElement` 和所有 `RoomPanelElement`
- 加载每个 panel 的 tri_mesh（使用 `load_geometry_with_enhanced_cache`）
- 将 panel 的 tri_mesh 转换到世界坐标系

#### 2.3 中心线与 tri_mesh 相交计算

- 对每个 BRAN 线段，与每个房间的 tri_mesh 进行相交检测
- 使用 Parry3D 的线段-tri_mesh 相交算法
- 计算相交区间：`[t_start, t_end]`（参数化表示）
- 转换为世界坐标：`intersection_start = start + dir * t_start`，`intersection_end = start + dir * t_end`
- 计算相交长度：`length = |intersection_end - intersection_start|`

#### 2.4 长度累加与主次房间判断

- 对每个房间，累加所有相交线段的长度
- 过滤：只保留 `total_length >= 1.0` 的房间
- 排序：按 `total_length` 降序排序
- 主房间：`is_primary = true`（仅第一个，即最长的）
- 次房间：`is_primary = false`（其余所有）

### 3. 数据库表设计

**表名**：`bran_room_intersection`

**字段定义**：

```sql
DEFINE TABLE bran_room_intersection SCHEMAFULL;

DEFINE FIELD bran_refno ON bran_room_intersection TYPE record<pe>;
DEFINE FIELD room_refno ON bran_room_intersection TYPE record<pe>;
DEFINE FIELD room_code ON bran_room_intersection TYPE string;
DEFINE FIELD total_length ON bran_room_intersection TYPE float;
DEFINE FIELD is_primary ON bran_room_intersection TYPE bool;
DEFINE FIELD confidence ON bran_room_intersection TYPE float;
DEFINE FIELD intersection_segments ON bran_room_intersection TYPE array;
DEFINE FIELD segment_count ON bran_room_intersection TYPE int;
DEFINE FIELD created_at ON bran_room_intersection TYPE datetime;
DEFINE FIELD updated_at ON bran_room_intersection TYPE datetime;
DEFINE FIELD calculation_version ON bran_room_intersection TYPE int;

DEFINE INDEX bran_room_idx ON bran_room_intersection FIELDS bran_refno, room_refno UNIQUE;
DEFINE INDEX bran_length_idx ON bran_room_intersection FIELDS bran_refno, total_length;
DEFINE INDEX room_bran_idx ON bran_room_intersection FIELDS room_refno, total_length;
```

**intersection_segments 数组结构**：

```json
{
  "segment_index": 0,
  "segment_start": [x1, y1, z1],
  "segment_end": [x2, y2, z2],
  "intersection_start": [x3, y3, z3],
  "intersection_end": [x4, y4, z4],
  "length": 2.5
}
```

### 4. 实现文件结构

```
rs-core/src/room/
├── bran_room_calc.rs          # 核心计算逻辑
│   ├── calculate_bran_room_intersection()  # 单个 BRAN 计算
│   ├── calculate_branches_room_intersection()  # 批量计算（10个）
│   ├── extract_bran_centerlines()  # 提取中心线
│   ├── load_room_tri_meshes()  # 加载房间 tri_mesh
│   ├── compute_segment_tri_mesh_intersection()  # 线段-tri_mesh 相交
│   └── determine_primary_room()  # 判断主次房间
│
├── bran_room_intersection.rs   # 数据结构定义
│   ├── BranRoomIntersection
│   ├── IntersectionSegment
│   └── BranRoomCalculationResult
│
└── bran_room_query.rs          # 查询接口
    ├── query_rooms_by_bran()
    ├── query_branches_by_room()
    └── query_primary_room_by_bran()

rs-core/src/rs_surreal/
└── bran_room.rs                # 数据库操作
    ├── insert_bran_room_intersections()
    ├── update_bran_room_intersections()
    ├── delete_bran_room_intersections()
    └── query_bran_room_intersections()
```

### 5. 增量更新策略

#### 5.1 BRAN 变更检测

- 监听 BRAN 几何数据变更事件
- 检测到变更时，标记该 BRAN 需要重新计算
- 批量处理：累积多个变更后统一重新计算

#### 5.2 房间变更检测

- 监听房间面板变更事件
- 检测到变更时，查询所有相关的 BRAN
- 重新计算这些 BRAN 的房间相交

#### 5.3 更新流程

1. 删除旧记录：`DELETE FROM bran_room_intersection WHERE bran_refno = $bran`
2. 重新计算：调用 `calculate_bran_room_intersection()`
3. 插入新记录：`INSERT INTO bran_room_intersection ...`

### 6. 性能优化

#### 6.1 批量处理优化

- 批量查询：一次查询多个 BRAN 的 segments
- 批量加载：并行加载多个房间的 tri_mesh
- 批量计算：使用线程池并行计算多个 BRAN

#### 6.2 空间索引优化

- 使用 SQLite R-tree 快速筛选候选房间
- 使用 AABB 预筛选，减少精确计算量

#### 6.3 缓存策略

- 缓存房间 tri_mesh（避免重复加载）
- 缓存 BRAN segments（短时间内不变）
- 缓存计算结果（设置 TTL）

### 7. API 接口设计

```rust
// 计算单个 BRAN 的房间相交
pub async fn calculate_bran_room_intersection(
    bran_refno: RefnoEnum,
) -> Result<Vec<BranRoomIntersection>>

// 批量计算多个 BRAN（最多 10 个）
pub async fn calculate_branches_room_intersection(
    bran_refnos: Vec<RefnoEnum>,
) -> Result<HashMap<RefnoEnum, Vec<BranRoomIntersection>>>

// 查询 BRAN 穿过的所有房间（按长度排序）
pub async fn query_rooms_by_bran(
    bran_refno: RefnoEnum,
) -> Result<Vec<BranRoomIntersection>>

// 查询 BRAN 的主房间
pub async fn query_primary_room_by_bran(
    bran_refno: RefnoEnum,
) -> Result<Option<BranRoomIntersection>>

// 查询房间内的所有 BRAN（按长度排序）
pub async fn query_branches_by_room(
    room_refno: RefnoEnum,
) -> Result<Vec<BranRoomIntersection>>

// 增量更新：BRAN 变更时重新计算
pub async fn update_bran_room_intersection(
    bran_refno: RefnoEnum,
) -> Result<Vec<BranRoomIntersection>>

// 增量更新：房间变更时重新计算相关 BRAN
pub async fn update_room_related_branches(
    room_refno: RefnoEnum,
) -> Result<usize>  // 返回更新的 BRAN 数量
```

### 8. 错误处理

- BRAN 无几何数据：返回空结果，记录警告
- 房间 tri_mesh 加载失败：跳过该房间，记录错误
- 相交计算失败：记录错误，继续处理其他房间
- 数据库操作失败：回滚事务，返回错误

### 9. 测试用例

1. **单元测试**：

   - 线段与 tri_mesh 相交计算
   - 主次房间判断逻辑
   - 长度累加计算

2. **集成测试**：

   - 单个 BRAN 计算流程
   - 批量 BRAN 计算（10个）
   - 增量更新流程

3. **性能测试**：

   - 10 个 BRAN 批量计算耗时
   - 内存使用情况
   - 数据库查询性能

## 三、实现步骤

1. **数据结构定义**（`bran_room_intersection.rs`）

   - 定义 `BranRoomIntersection`、`IntersectionSegment` 等结构
   - 实现序列化/反序列化

2. **核心计算逻辑**（`bran_room_calc.rs`）

   - 实现中心线提取
   - 实现 tri_mesh 加载
   - 实现线段-tri_mesh 相交计算
   - 实现主次房间判断

3. **数据库操作**（`bran_room.rs`）

   - 创建表结构
   - 实现 CRUD 操作

4. **查询接口**（`bran_room_query.rs`）

   - 实现查询 API

5. **增量更新**（集成到现有事件系统）

   - 监听 BRAN/房间变更事件
   - 实现更新逻辑

6. **测试验证**

   - 编写单元测试
   - 编写集成测试
   - 性能测试

## 四、关键技术点

1. **Parry3D 线段-tri_mesh 相交**：

   - 使用 `parry3d::query::intersection_test` 或自定义算法
   - 处理线段端点、中间点与 tri_mesh 的相交

2. **世界坐标转换**：

   - Panel transform 应用到 tri_mesh
   - BRAN segment 已经是世界坐标

3. **精度处理**：

   - 浮点误差容差：1e-6
   - 边界情况：线段在房间边界上

4. **并发安全**：

   - 使用 `Arc` 共享 tri_mesh
   - 使用 `DashMap` 存储中间结果

### To-dos

- [ ] 定义数据结构：BranRoomIntersection、IntersectionSegment、BranRoomCalculationResult
- [ ] 实现 BRAN 中心线提取：从 PipelineSegmentRecord 提取 PipelineSpan 并转换为线段
- [ ] 实现房间 tri_mesh 加载：使用空间索引查询候选房间，加载所有 panel 的 tri_mesh
- [ ] 实现线段-tri_mesh 相交计算：使用 Parry3D 计算精确相交区间和长度
- [ ] 实现主次房间判断：累加长度，过滤 >= 1.0 米，排序后标记主房间
- [ ] 创建数据库表结构：bran_room_intersection 表及索引
- [ ] 实现数据库 CRUD 操作：插入、更新、删除、查询 bran_room_intersection 记录
- [ ] 实现批量计算接口：支持最多 10 个 BRAN 的批量计算
- [ ] 实现查询接口：query_rooms_by_bran、query_primary_room_by_bran、query_branches_by_room
- [ ] 实现增量更新机制：监听 BRAN/房间变更事件，自动重新计算
- [ ] 编写单元测试：测试线段-tri_mesh 相交、主次房间判断等核心逻辑
- [ ] 编写集成测试：测试完整计算流程和批量处理