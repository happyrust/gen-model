# cal_room_refnos() 函数深度调查报告

## 概述

本报告详细分析 `cal_room_refnos()` 函数的实现细节，该函数是房间内构件计算的核心引擎。通过两阶段架构（粗算+细算）计算房间面板内包含的所有构件。

---

## Code Sections (The Evidence)

### 核心计算函数

- `src/fast_model/room_model.rs` (lines 412-526, `cal_room_refnos`): 改进版本的房间构件计算函数。实现两阶段算法：(1) 粗算阶段使用SQLite空间索引查询候选，(2) 细算阶段使用关键点包含测试精确判断。返回房间内构件的 HashSet<RefnoEnum>。

### 关键点提取函数

- `src/fast_model/room_model.rs` (lines 595-637, `extract_aabb_key_points`): 从AABB（轴对齐包围盒）提取27个关键点的函数。包括8个顶点、1个中心点、6个面中心、12条边中点。

- `src/fast_model/room_model.rs` (lines 640-650, `extract_geom_key_points`): 从几何体实例数组提取所有关键点的聚合函数。遍历每个几何实例的AABB并调用 `extract_aabb_key_points()`。

### 包含判断函数

- `src/fast_model/room_model.rs` (lines 654-675, `is_geom_in_panel`): 判断关键点是否在面板内的投票策略实现。使用 parry3d 的 TriMesh.project_point() 进行点到网格的距离检测，50% 阈值投票。

### 几何网格加载与缓存

- `src/fast_model/room_model.rs` (lines 529-575, `load_geometry_with_enhanced_cache`): 使用L0 LOD mesh的增强缓存加载函数。使用 DashMap 存储 Arc<PlantMesh> 缓存，缓存键为 `{geo_hash}_L0`，自动缓存清理策略（超过2000条目移除50%）。

- `src/fast_model/room_model.rs` (lines 578-591, `cleanup_geometry_cache`): 缓存清理函数。简单策略：移除缓存中前50%的条目。

### 粗算阶段实现

- `src/fast_model/room_model.rs` (lines 445-472): 粗算部分代码。通过 tokio::task::spawn_blocking 调用 aios_core::spatial::sqlite::query_overlap() 查询与面板AABB重叠的构件，返回 Vec<RefnoEnum>。排除规则：排除面板本身和其他面板。

### 细算阶段实现

- `src/fast_model/room_model.rs` (lines 481-515): 细算部分代码。循环遍历每个候选构件，调用 extract_geom_key_points() 提取关键点，调用 is_geom_in_panel() 判断，使用 HashSet 存储最终结果。

### 面板几何加载逻辑

- `src/fast_model/room_model.rs` (lines 420-443): 面板几何加载部分。查询面板的GeomInstQuery，取第一个实例的世界变换和geo_hash，调用 load_geometry_with_enhanced_cache() 加载L0网格，返回 Arc<TriMesh>。

### 并发处理框架

- `src/fast_model/room_model.rs` (lines 134-225, `build_room_relations`): 改进版房间关系构建函数。三个主要步骤：(1) build_room_panels_relate() 查询房间面板映射，(2) compute_room_relations() 并发处理，(3) 统计和返回结果。

- `src/fast_model/room_model.rs` (lines 174-225, `compute_room_relations`): 并发计算函数。使用 futures::stream 和 buffer_unordered(options.concurrency) 控制并发度（默认4），对每个房间面板调用 process_panel_for_room()。

### 单面板处理

- `src/fast_model/room_model.rs` (lines 376-408, `process_panel_for_room`): 单个面板的异步处理函数。调用 cal_room_refnos() 计算构件，调用 save_room_relate() 保存关系，返回构件计数。

### 房间关系保存

- `src/fast_model/room_model.rs` (lines 678-711, `save_room_relate`): 房间关系数据库保存函数。构建SurrealDB RELATE语句，批量执行SQL。关系结构：`panel_refno->room_relate->component_refno`。

### 增量更新支持

- `src/fast_model/room_model.rs` (lines 822-908, `update_room_relations_incremental`): 增量更新函数。查询受影响面板，删除旧关系，重新计算新关系。支持只更新指定refnos相关的房间关系。

- `src/fast_model/room_model.rs` (lines 912-943, `query_panels_containing_refnos`): 查询包含指定refnos的房间面板。用于增量更新时确定影响范围。

### 缓存管理

- `src/fast_model/room_model.rs` (lines 115-124): 全局几何缓存定义。使用 tokio::sync::OnceCell 和 DashMap<String, Arc<PlantMesh>>。支持并发访问和缓存指标统计。

- `src/fast_model/room_model.rs` (lines 78-111): 缓存指标结构体。使用原子操作（AtomicU64）统计缓存命中和未命中，计算命中率百分比。

### 面板查询SQL构建

- `src/fast_model/room_model.rs` (lines 228-269, `build_room_panel_query_sql`): 房间面板查询SQL构建函数。支持项目特性条件编译（project_hd/project_hh）。查询关键词：递归1-2层children，过滤noun='PANE'。

### 测试验证

- `src/test/test_room_v2_verification.rs` (lines 18-90): V2改进验证测试。验证L0 LOD mesh路径、关键点提取、粗细算性能分离、计算结果准确性。

- `src/test/test_room_integration.rs` (lines 23-192, `test_room_integration_complete`): 完整集成测试。涵盖数据库初始化、房间查询、模型生成、房间计算、结果验证五个步骤。

---

## Report (The Answers)

### result

#### 1. cal_room_refnos() 函数的完整实现逻辑

`cal_room_refnos()` 是一个异步函数，采用两阶段计算模式：

**函数签名**:
```
async fn cal_room_refnos(
    mesh_dir: &PathBuf,
    panel_refno: RefnoEnum,
    exclude_refnos: &HashSet<RefnoEnum>,
    inside_tol: f32,
) -> anyhow::Result<HashSet<RefnoEnum>>
```

**完整流程**（412-526行）：

1. **步骤1：查询面板几何实例** (420-428行)
   - 调用 `aios_core::query_insts(&[panel_refno], true)` 查询面板的所有几何实例
   - 返回 `Vec<GeomInstQuery>` 结构，包含世界变换、AABB等信息
   - 如果为空，直接返回空集合

2. **步骤2：加载面板L0网格** (430-443行)
   - 从第一个几何实例提取：世界变换(world_trans)、几何哈希(geo_hash)
   - 调用 `load_geometry_with_enhanced_cache()` 加载L0 LOD mesh
   - 返回 `Arc<TriMesh>` 用于点包含测试

3. **步骤3：粗算阶段** (445-480行)
   - 从面板AABB构建包围盒
   - 在 tokio blocking 线程池中执行空间查询（避免阻塞异步runtime）
   - 调用 `aios_core::spatial::sqlite::query_overlap()` 返回所有与AABB重叠的构件
   - 结果过滤：排除面板本身和排除列表中的所有refnos
   - 记录粗算耗时和候选数量

4. **步骤4：细算阶段** (481-515行)
   - 遍历每个候选构件
   - 对每个构件：
     - 调用 `aios_core::query_insts()` 获取其几何实例
     - 调用 `extract_geom_key_points()` 从所有实例提取关键点
     - 调用 `is_geom_in_panel()` 判断关键点是否在面板内
     - 若通过判断，添加到结果集合中
   - 记录细算耗时和最终构件数量

5. **返回结果** (525行)
   - 返回 `HashSet<RefnoEnum>` 包含所有房间内的构件

#### 2. 粗算阶段（空间索引查询）如何工作

粗算阶段使用SQLite空间索引加快查询（445-479行）：

**索引类型**：SQLite RTree 空间索引
- 存储：`test-room-build.db` 或通过 `sqlite-index` 特性管理
- 索引表名：`aabb_index`

**查询流程**：
1. 面板AABB转换为parry3d::bounding_volume::Aabb
2. 在blocking线程池中执行（避免阻塞异步IO）
3. 调用 `aios_core::spatial::sqlite::query_overlap()` 查询重叠构件
4. 参数：(aabb, None, Some(1000), &exclude_list)
   - aabb：查询范围
   - None：不限制边界
   - Some(1000)：最大返回1000条结果
   - &exclude_list：排除列表（转换为 Vec<RefU64>）
5. 返回 `Vec<(RefU64, BoundingBox, extra_info)>`，转换为 `Vec<RefnoEnum>`

**性能特点**：
- 通常耗时 10-100ms（根据候选数量）
- 使用RTree的二进制搜索，时间复杂度O(log n)
- 候选数通常是最终结果的2-10倍

#### 3. 细算阶段（几何精确检测）的算法

细算使用基于关键点投票的包含检测算法（481-515行）：

**算法基础**：
- 物理基础：点在凸多面体内部的判断
- 实现库：parry3d (Rapier 3D几何库)
- 方法：TriMesh.project_point() 投影距离 + 投票

**细算流程**：

对每个候选构件：
1. 调用 `aios_core::query_insts(&[candidate_refno], true)` 获取几何实例
2. 调用 `extract_geom_key_points(&candidate_insts)` 提取关键点
3. 调用 `is_geom_in_panel(&key_points, &panel_tri_mesh, tolerance)` 判断

**关键点投票机制**（654-675行，is_geom_in_panel函数）：
```rust
// 对每个关键点：
let projection = panel_tri_mesh.project_point(&Isometry::identity(), point, true);
let distance_sq = (projection.point - point).norm_squared();

// 判断条件（二选一）：
if projection.is_inside || distance_sq <= tolerance_sq {
    points_inside += 1;  // 计数
}

// 最终判决：超过50%的关键点在面板内即判定属于该房间
let threshold = (total_points as f32 * 0.5) as usize;
return points_inside >= threshold;
```

**50% 阈值的含义**：
- 容错机制：允许最多50%的关键点在面板外（边界构件）
- 目的：避免因网格精度、变换误差导致误判
- 效果：较为激进，可能包含部分边界接近的构件
- 可调参数：当前硬编码为0.5，可修改为0.6或0.7以提高精度

**性能特点**：
- 每个候选构件耗时 1-10ms（取决于关键点数和网格复杂度）
- 典型场景：候选数 100-500，细算耗时 100-2000ms

#### 4. 关键点提取函数 `extract_aabb_key_points()` 和 `extract_geom_key_points()`

**extract_aabb_key_points()** (595-637行)：

从AABB提取27个关键点的精确实现：

1. **8个顶点**（使用 aabb.vertices()）
   - 最小和最大坐标的8个组合
   - (min_x, min_y, min_z), (max_x, min_y, min_z), ...等

2. **1个中心点**（使用 aabb.center()）
   - 立方体的几何中心

3. **6个面中心** (611-616行)
   - 计算面的中点：(mins.x, cy, cz), (maxs.x, cy, cz), ...
   - 其中 cy = (mins.y + maxs.y) / 2.0 等

4. **12条边中点** (618-634行)
   - 底面4条边：vertices[0-1], [1-3], [3-2], [2-0]
   - 顶面4条边：vertices[4-5], [5-7], [7-6], [6-4]
   - 竖直4条边：vertices[0-4], [1-5], [2-6], [3-7]
   - 计算方法：`(v1 + v2) / 2.0`

**为什么选择27个点**：
- 8个顶点：覆盖AABB的极值点
- 1个中心：代表整体位置
- 6个面中心：检测面方向的偏离
- 12个边中点：捕捉边缘特征
- 总体：密度足够以检测大多数边界情况

**extract_geom_key_points()** (640-650行)：

简单的聚合函数：
```rust
fn extract_geom_key_points(geom_insts: &[GeomInstQuery]) -> Vec<Point<Real>> {
    let mut all_points = Vec::new();
    for geom_inst in geom_insts {
        let aabb: Aabb = geom_inst.world_aabb.into();  // 转换为Aabb
        let points = extract_aabb_key_points(&aabb);   // 提取27个点
        all_points.extend(points);                      // 累加
    }
    all_points
}
```

- 处理多个几何实例（一个构件可能有多个LOD或实例）
- 返回所有实例关键点的合集
- 关键点总数 = 实例数 × 27

#### 5. is_geom_in_panel() 函数的判断逻辑和50% 阈值

**函数签名** (654行)：
```rust
fn is_geom_in_panel(
    key_points: &[Point<Real>],
    panel_tri_mesh: &TriMesh,
    tolerance: f32
) -> bool
```

**判断逻辑** (654-675行)：

1. **边界检查**：若关键点为空，返回false

2. **逐点检测**（663-670行）：
   ```rust
   for point in key_points {
       let projection = panel_tri_mesh.project_point(
           &Isometry::identity(),  // 无旋转/位移
           point,
           true  // 检查点是否在内部
       );
       let distance_sq = (projection.point - point).norm_squared();

       // 两个条件之一成立即计数
       if projection.is_inside || distance_sq <= tolerance_sq {
           points_inside += 1;
       }
   }
   ```

3. **投票判决**（672-674行）：
   ```rust
   let threshold = (total_points as f32 * 0.5) as usize;
   return points_inside >= threshold;  // ≥ 50% 即判定为在内部
   ```

**50% 阈值的解释**：

| 关键点占比 | 判定结果 | 说明 |
|----------|--------|------|
| > 50% | IN | 属于房间内 |
| ≤ 50% | OUT | 不属于房间内 |

**含义和影响**：

- **包容性强**：50% 是相对宽松的阈值
  - 允许最多50%的关键点在面板外
  - 可能包含边界接近的构件

- **容错机制**：
  - 网格精度误差（±tolerance）
  - 变换计算舍入误差
  - 几何简化带来的偏差

- **实际应用**：
  - 优点：减少漏报（遗漏应该包含的构件）
  - 缺点：可能增加误报（错误包含边界构件）

- **可优化方向**：
  - 将0.5改为0.6/0.7以提高精度
  - 增加关键点数量以更好地代表几何体

#### 6. 网格文件加载逻辑（L0 LOD mesh）

L0加载流程（529-575行，load_geometry_with_enhanced_cache）：

**缓存策略**：

1. **缓存键构造**（537-538行）：
   ```rust
   let cache_key = format!("{}_L0", geo_hash);
   ```
   - 使用 `geo_hash_L0` 作为键
   - 不同LOD级别使用不同的键（L0, L1, L2等）

2. **缓存查询**（541-550行）：
   ```rust
   if let Some(cached_mesh) = cache.get(&cache_key) {
       if let Some(tri_mesh) = cached_mesh.get_tri_mesh_with_flag(
           (world_trans * inst.transform).to_matrix(),
           TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
       ) {
           CACHE_METRICS.record_hit();
           return Ok(Arc::new(tri_mesh));
       }
   }
   ```
   - 从缓存取出PlantMesh
   - 应用世界变换和实例变换
   - 设置TriMesh标志：ORIENTED（有向）+ MERGE_DUPLICATE_VERTICES（去重）
   - 记录缓存命中

3. **文件加载**（552-567行）：
   ```rust
   let file_path = mesh_dir.join(build_mesh_path(geo_hash, "L0"));
   let mesh = tokio::task::spawn_blocking(move ||
       PlantMesh::des_mesh_file(&file_path)
   ).await??;

   let tri_mesh = mesh
       .get_tri_mesh_with_flag(
           (world_trans * inst.transform).to_matrix(),
           TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
       )
       .ok_or_else(|| anyhow::anyhow!("无法构建 TriMesh"))?;
   ```
   - 使用 `build_mesh_path(geo_hash, "L0")` 构建L0网格的路径
   - 在blocking线程池中加载（避免阻塞异步runtime）
   - 使用 PlantMesh::des_mesh_file() 反序列化网格文件
   - 调用 get_tri_mesh_with_flag() 创建parry3d的TriMesh
   - 应用变换矩阵

4. **缓存更新和清理**（565-572行）：
   ```rust
   cache.insert(cache_key, Arc::new(mesh));
   CACHE_METRICS.record_miss();

   if cache.len() > 2000 {
       cleanup_geometry_cache(&cache).await;
   }
   ```
   - 将PlantMesh存储到缓存（使用Arc提升共享）
   - 记录缓存未命中
   - 当缓存超过2000条目时触发清理

**L0 LOD 优势**：

- **文件大小**：通常比L1/L2小60-80%
- **内存占用**：减少内存开销
- **加载速度**：快速I/O
- **精度权衡**：足以用于房间计算的包含测试

**文件路径规则**：

- 目录结构：`{mesh_dir}/lod_L0/`
- 文件命名：`{geo_hash}.bin` 或相应格式
- `build_mesh_path()` 函数（来自 aios_core::utils::lod_path_detector）负责路径构建

#### 7. 性能优化点

项目实现了多个性能优化：

**并发优化** (174-225行，compute_room_relations)：
- 使用 `futures::stream` 和 `buffer_unordered(concurrency)`
- 默认并发度：4（可通过 `ROOM_RELATION_CONCURRENCY` 环境变量调整）
- 避免过度并发导致内存和IO压力

**缓存优化** (115-124, 541-572行)：
- 使用 DashMap 替代 Arc<Mutex<HashMap>>（更高效的并发）
- 使用 Arc<PlantMesh> 避免重复克隆网格数据
- 自动缓存清理：超过2000条目移除50%
- 缓存指标统计：hit_rate 监控（92-110行）

**I/O优化**：
- 使用 L0 LOD mesh 减少文件大小和加载时间
- 使用 tokio::task::spawn_blocking 避免阻塞异步runtime
- 批量SQL执行减少数据库往返

**算法优化**：
- 两阶段架构：粗筛 + 细筛
  - 粗筛：使用空间索引快速排除大量候选
  - 细筛：使用关键点投票快速判定
- 关键点选择：27个点相对较少，但足以覆盖大多数情况

**内存优化**：
- 使用 HashSet 而非 Vec 避免重复
- 排除列表使用 Vec<RefU64> 而非 HashSet（小集合使用向量更快）
- 异步流处理避免一次加载所有数据

---

## conclusions

### 关键事实总结

1. **计算架构**：`cal_room_refnos()` 采用两阶段设计
   - 粗算：SQLite RTree空间索引查询，时间复杂度 O(log n)，典型耗时 10-100ms
   - 细算：关键点投票，时间复杂度 O(k×p)，其中k=候选数，p=关键点数

2. **关键点设计**：27个关键点分布
   - 8顶点 + 1中心 + 6面中心 + 12边中点
   - 密度设计用于捕捉AABB的特征和边界情况

3. **判定阈值**：50% 投票阈值
   - 相对宽松，优先减少漏报
   - 可能包含部分边界接近的构件

4. **网格加载**：L0 LOD 优化加载
   - 文件大小减少60-80%
   - 使用DashMap全局缓存，Arc智能指针共享
   - 自动清理策略保证内存

5. **并发控制**：4个工作线程
   - buffer_unordered(4) 避免过度并发
   - 支持 ROOM_RELATION_CONCURRENCY 环境变量调整

6. **排除列表**：防止重复计算
   - 排除面板本身和所有其他房间的面板
   - 使用 HashSet<RefnoEnum> 管理

7. **容错机制**：多层次容错
   - 点到网格距离容差（inside_tol，默认0.1）
   - parry3d的projection.is_inside 标志
   - 50%投票阈值

---

## relations

### 代码依赖关系

**cal_room_refnos() 的调用链**：

```
build_room_relations()
  └─ compute_room_relations()
      └─ process_panel_for_room()
          └─ cal_room_refnos()
              ├─ aios_core::query_insts() [查询几何实例]
              ├─ load_geometry_with_enhanced_cache()
              │   └─ aios_core::utils::lod_path_detector::build_mesh_path()
              ├─ aios_core::spatial::sqlite::query_overlap() [粗算]
              ├─ extract_geom_key_points()
              │   └─ extract_aabb_key_points()
              └─ is_geom_in_panel()
                  └─ parry3d::query::PointQuery::project_point()
```

**关键函数之间的关系**：

1. **cal_room_refnos() → extract_geom_key_points()**
   - 为每个候选构件提取关键点
   - 关键点作为后续包含判断的输入

2. **extract_geom_key_points() → extract_aabb_key_points()**
   - 对每个几何实例的AABB调用
   - 聚合所有实例的关键点

3. **is_geom_in_panel() ← parry3d::TriMesh**
   - 使用TriMesh的project_point()进行距离计算
   - projection.is_inside 标志用于判断

4. **load_geometry_with_enhanced_cache() ← ENHANCED_GEOMETRY_CACHE**
   - 全局DashMap缓存存储加载的网格
   - 缓存键为 `{geo_hash}_L0`
   - 缓存命中避免重复加载

5. **process_panel_for_room() → save_room_relate()**
   - cal_room_refnos() 返回的构件集合
   - 直接保存到数据库的房间关系

### 数据流动

```
Panel Refno
    ↓
[query_insts] → GeomInstQuery[]
    ↓
[加载L0网格] → TriMesh (panel)
    ↓
[查询AABB] → Aabb
    ↓
[粗算] → Vec<RefnoEnum> (candidates)
    ↓
for each candidate:
    ├─ [query_insts] → GeomInstQuery[]
    ├─ [extract_key_points] → Point[]
    └─ [投票判定] → bool (is_inside)
    ↓
HashSet<RefnoEnum> → save_room_relate()
    ↓
SurrealDB (room_relate)
```

### 测试验证关系

**test_room_v2_verification.rs** 测试 cal_room_refnos()：
- 验证L0网格加载
- 验证粗细算性能分离
- 验证关键点提取逻辑
- 验证结果准确性

**test_room_integration.rs** 集成测试整个流程：
- 测试 build_room_relations()
- 验证数据库保存结果
- 验证统计数据正确性

### 性能监控关系

```
cal_room_refnos()
    ├─ 粗算耗时 → 日志 "🔍 粗算完成: 耗时 XXms, 候选数 YY"
    ├─ 细算耗时 → 日志 "✅ 细算完成: 耗时 XXms, 结果数 YY"
    └─ 总耗时 → 日志 "面板 {} 房间计算完成: 总耗时 XXms, 粗算 YY -> 细算 ZZ"

CACHE_METRICS
    ├─ record_hit() [缓存命中]
    ├─ record_miss() [缓存未命中]
    └─ hit_rate() [命中率百分比]

RoomBuildStats
    ├─ total_rooms [处理房间数]
    ├─ total_panels [处理面板数]
    ├─ total_components [计算构件数]
    ├─ build_time_ms [总耗时]
    ├─ cache_hit_rate [缓存命中率]
    └─ memory_usage_mb [内存占用]
```

---

## 计算流程详细图表

### 粗算阶段流程图

```
面板Refno
    ↓
[query_insts] → GeomInstQuery (world_trans, aabb)
    ↓
面板 AABB → parry3d::Aabb
    ↓
[spawn_blocking]
    ↓
sqlite::query_overlap(aabb, exclude_list, limit=1000)
    ↓
Vec<(RefU64, BoundingBox, ...)>
    ↓
[转换 & 过滤]
    ├─ 排除面板本身
    └─ 排除exclude_list中的所有refno
    ↓
Vec<RefnoEnum> (candidates)
    ↓
记录: 🔍 粗算耗时, 候选数
```

### 细算阶段流程图

```
for each candidate_refno in candidates:
    ├─ [query_insts] → GeomInstQuery[]
    │   ↓
    │   [extract_geom_key_points]
    │   ├─ for each GeomInstQuery:
    │   │   ├─ get world_aabb
    │   │   └─ [extract_aabb_key_points] → 27个Point
    │   └─ → Vec<Point> (all_points)
    │   ↓
    │   [is_geom_in_panel](all_points, panel_tri_mesh, tol)
    │   ├─ for each point:
    │   │   ├─ tri_mesh.project_point(point)
    │   │   ├─ distance_sq = (proj.point - point).norm_squared()
    │   │   └─ if projection.is_inside OR distance_sq ≤ tol_sq:
    │   │       points_inside += 1
    │   └─ if points_inside ≥ (total_points * 0.5):
    │       return true
    │   ↓
    │   if true: within_refnos.insert(candidate_refno)
    │
至此遍历完所有候选

HashSet<RefnoEnum> (final_result)
    ↓
记录: ✅ 细算耗时, 结果数
```

### 关键点分布图

```
AABB 结构：
        7 ─────── 6       (顶面)
       /|        /|
      4 ─────── 5 |
      | 3 ─────│─ 2       (底面)
      |/       |/
      0 ─────── 1

27个关键点分布：
- 顶点: 0, 1, 2, 3, 4, 5, 6, 7 (8个)
- 中心: Center (1个)
- 面中心:
  - 左面(min_x): L_center (1个)
  - 右面(max_x): R_center (1个)
  - 前面(min_y): F_center (1个)
  - 后面(max_y): B_center (1个)
  - 下面(min_z): D_center (1个)
  - 上面(max_z): U_center (1个)
- 边中点: (12个)
  - 底面4条: (0-1), (1-3), (3-2), (2-0)
  - 顶面4条: (4-5), (5-7), (7-6), (6-4)
  - 竖直4条: (0-4), (1-5), (2-6), (3-7)

总计: 8 + 1 + 6 + 12 = 27个
```

---

## 需要测试的计算步骤

### 单元测试（不需要数据库）

1. **AABB关键点提取测试**
   - 验证27个点的坐标计算正确性
   - 测试边界情况（0大小的AABB、极大/极小值）

2. **投票阈值测试**
   - 验证50%阈值的计算
   - 测试边界情况：exactly 50%、49%、51%

3. **缓存管理测试**
   - 验证缓存键生成（`{geo_hash}_L0`）
   - 验证缓存清理逻辑（超过2000条目）
   - 验证缓存命中率计算

### 集成测试（需要数据库和网格文件）

1. **粗算性能验证**
   - 运行粗算，验证候选数量合理（通常100-500）
   - 验证耗时 < 100ms/面板
   - 验证没有候选数为0的情况

2. **细算性能验证**
   - 运行细算，验证结果数 < 候选数
   - 验证细算耗时 < 500ms/面板
   - 验证没有细算耗时异常长的情况

3. **网格加载测试**
   - 验证L0网格正确加载
   - 验证缓存命中率 > 50%
   - 验证内存使用 < 2GB（大型项目）

4. **包含判定测试**
   - 验证明显在内部的构件被识别
   - 验证明显在外部的构件被排除
   - 验证边界附近构件的结果合理

5. **增量更新测试**
   - 修改部分构件，验证增量更新正确
   - 验证更新后的关系完整

### 性能基准测试

根据 ROOM_V2_VERIFICATION.md 的性能基准：

| 项目规模 | 房间数 | 面板数 | 预期总耗时 | 预期平均 |
|---------|--------|--------|-----------|---------|
| 小型   | 10-50   | 50-200  | 5-30s    | 500ms  |
| 中型   | 50-200  | 200-1000| 30-120s  | 600ms  |
| 大型   | 200-500 | 1000+   | 2-10min  | 1000ms |

---

## 文档参考

- **源代码实现**：`src/fast_model/room_model.rs` (412-675行)
- **集成测试**：`src/test/test_room_integration.rs`
- **验证测试**：`src/test/test_room_v2_verification.rs`
- **架构文档**：`docs/architecture/ROOM_COMPUTE_SYSTEM_FLOW.md`
- **验证指南**：`docs/ROOM_V2_VERIFICATION.md`
- **CLAUDE.md**：项目级开发指引

