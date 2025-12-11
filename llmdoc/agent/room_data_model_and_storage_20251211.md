<!-- 房间计算的数据模型和存储结构深度调查报告 -->

### Code Sections (The Evidence)

#### 房间数据结构定义

- `src/fast_model/room_model.rs` (lines 44-52, `RoomBuildStats`): 房间关系构建统计信息结构。字段包括：total_rooms (房间总数), total_panels (面板总数), total_components (构件总数), build_time_ms (构建耗时毫秒), cache_hit_rate (缓存命中率), memory_usage_mb (内存使用MB数)。

- `src/fast_model/room_model.rs` (lines 55-68, `RoomComputeOptions`): 房间计算配置结构。字段包括：inside_tol (点包含容差值，默认0.1), concurrency (并发度，默认从环境变量ROOM_RELATION_CONCURRENCY读取或为4)。

- `src/fast_model/room_model.rs` (lines 78-127, `CacheMetrics`): 缓存性能指标结构，使用AtomicU64统计。字段包括：plant_hits (PlantMesh缓存命中数), plant_misses (PlantMesh缓存未命中数), trimesh_hits (TriMesh缓存命中数), trimesh_misses (TriMesh缓存未命中数)。支持hit_rate()计算命中率。

#### 房间面板关系数据结构

- `src/fast_model/room_model.rs` (lines 202-206, `build_room_relations` 返回值): 房间面板映射为Vec<(RefnoEnum, String, Vec<RefnoEnum>)>，三元组表示(房间参考号, 房间号字符串, 面板参考号列表)。

- `src/fast_model/room_model.rs` (lines 330-370): 房间面板查询和转换逻辑。通过SurrealDB QUERY查询FRMW->SBFR->PANE层级关系，返回(room_thing, room_num, panel_things)三元组，其中room_thing为RecordId，room_num为房间号字符串，panel_things为RecordId列表。

#### 房间关系存储结构（SurrealDB）

- `src/fast_model/room_model.rs` (lines 728-735): 房间关系SurrealDB存储格式。关系类型为"room_relate"，存储命令为 `relate {panel_refno}->room_relate:{relation_id}->{component_refno} set room_num='{}', confidence=0.9, created_at=time::now();` 。关系属性包括room_num (房间号字符串)和confidence (置信度，固定0.9)。

- `src/fast_model/room_model.rs` (lines 390-395): 房间与面板的关系。构建 `relate {room_refno}->room_panel_relate->[{panel_refnos}] set room_num='{}';` 语句，建立房间到面板的直接关系。

#### 房间号查询表（MySQL）

- `src/tables.rs` (lines 230-236, `gen_create_room_code_table_sql`): ROOM_CODE表定义。表结构：REFNO (BIGINT), ROOM_NAME (VARCHAR(50))。用于存储房间参考号与房间号的映射。

- `src/api/room_code.rs` (lines 55-71): ROOM_CODE表查询函数。支持单个查询 `SELECT ROOM_NAME FROM ROOM_CODE WHERE REFNO = {refno}` 和批量查询 `SELECT REFNO,ROOM_NAME FROM ROOM_CODE WHERE REFNO IN (...)`。

#### AABB 空间索引存储结构（SQLite）

- `src/fast_model/aabb_cache.rs` (lines 13-17, `StoredAabb`): AABB序列化存储结构。字段包括：mins (最小点[x,y,z]float数组), maxs (最大点[x,y,z]float数组)。

- `src/fast_model/aabb_cache.rs` (lines 180-246, `init_schema`): SQLite表结构定义。包含多个表：
  - ref_bbox (refno INTEGER PRIMARY KEY, data BLOB) - 存储主要AABB数据
  - geo_aabb (geo_hash TEXT PRIMARY KEY, data BLOB) - 几何体AABB存储
  - deps_by_ref (refno INTEGER PRIMARY KEY, data BLOB) - 参考号依赖关系
  - refs_by_geo (geo_hash TEXT PRIMARY KEY, data BLOB) - 几何体依赖的参考号
  - versioned_ref_bbox (refno_key TEXT, session INTEGER, data BLOB, PRIMARY KEY(refno_key, session)) - 版本化AABB存储
  - refno_time_data (refno_key TEXT, session INTEGER, data BLOB, PRIMARY KEY(refno_key, session)) - 时间戳数据
  - sesno_time_mapping (dbnum INTEGER, sesno INTEGER, timestamp INTEGER, description TEXT, PRIMARY KEY(dbnum, sesno)) - 会话时间映射
  - aabb_index (VIRTUAL TABLE USING rtree(id, min_x, max_x, min_y, max_y, min_z, max_z)) - 3D R*-tree空间索引

- `src/fast_model/aabb_cache.rs` (lines 120-141, `StoredRStarBBox`): R*-tree存储结构。字段包括：aabb (StoredAabb), refno (u64), noun (String)。用于序列化保存到SQLite。

#### AABB 版本化存储

- `src/fast_model/aabb_cache.rs` (lines 20-28, `VersionedStoredAabb`): 版本化AABB数据结构。字段包括：refno_value (u64), session (u32), mins/maxs ([f32;3]), created_at (u64秒级时间戳), updated_at (u64秒级时间戳)。

- `src/fast_model/aabb_cache.rs` (lines 31-41, `RefnoTimeData`): 时间数据结构。字段包括：refno_value (u64), session (u32), dbnum (u32), created_at (创建时间戳), updated_at (更新时间戳), sesno_timestamp (sesno对应实际时间), author (创建者), description (变更描述)。

#### 房间计算的几何数据结构

- `src/fast_model/room_model.rs` (lines 595-637, `extract_aabb_key_points`): 从AABB提取27个关键点。包括8个顶点、1个中心点、6个面中心、12条边中点。使用parry3d::math::Point<Real>表示。

- `src/fast_model/room_model.rs` (lines 640-650, `extract_geom_key_points`): 聚合几何体关键点提取函数。参数为Vec<GeomInstQuery>，返回Vec<Point<Real>>。对每个几何实例的world_aabb提取关键点。

#### 房间构件计算核心算法

- `src/fast_model/room_model.rs` (lines 412-526, `cal_room_refnos`): 房间构件计算函数，返回HashSet<RefnoEnum>。四阶段算法：
  1. 加载面板几何（lines 420-443）：查询面板GeomInstQuery，获取world_transform和geo_hash，加载L0 LOD网格
  2. 粗算-空间索引查询（lines 445-479）：使用aios_core::spatial::sqlite::query_overlap()查询与面板AABB重叠的候选构件
  3. 细算-关键点检测（lines 481-515）：提取候选构件关键点，调用is_geom_in_panel()精确判断
  4. 返回最终结果（line 526）：返回HashSet<RefnoEnum>

- `src/fast_model/room_model.rs` (lines 654-675, `is_geom_in_panel`): 点包含判定函数。使用parry3d::shape::TriMesh::project_point()进行距离检测，阈值0.1。采用50%投票策略：超过50%的关键点在面板内则判定为包含。

#### 增强的几何缓存机制

- `src/fast_model/room_model.rs` (lines 131-136): 全局几何缓存定义。两个静态变量：
  - ENHANCED_GEOMETRY_CACHE: tokio::sync::OnceCell<DashMap<String, Arc<PlantMesh>>> - PlantMesh缓存
  - ENHANCED_TRIMESH_CACHE: tokio::sync::OnceCell<DashMap<String, Arc<TriMesh>>> - TriMesh缓存（L0 LOD，未应用变换）

- `src/fast_model/room_model.rs` (lines 529-576, `load_geometry_with_enhanced_cache`): 缓存加载函数。缓存键格式为"{geo_hash}_L0"。命中时从缓存的PlantMesh构建TriMesh，未命中时从磁盘调用PlantMesh::des_mesh_file()。TriMesh标志为ORIENTED | MERGE_DUPLICATE_VERTICES。

#### 房间关系保存

- `src/fast_model/room_model.rs` (lines 716-749, `save_room_relate`): 房间关系保存函数。对每个构件生成RELATE语句，relation_id为"{panel_refno}_{component_refno}"。批量执行所有SQL语句。

- `src/fast_model/room_model.rs` (lines 376-408, `process_panel_for_room`): 单个面板处理函数。调用cal_room_refnos()计算构件，调用save_room_relate()保存关系，返回成功保存的构件数。

#### 房间关系更新和查询

- `src/fast_model/room_model.rs` (lines 1519-1669, `update_room_relations_incremental`): 增量更新函数。输入受影响的refnos，查询包含这些refnos的房间面板，删除旧关系，重新计算新关系。返回RoomRelationUpdateResult结构。

- `src/fast_model/room_model.rs` (lines 1612-1637, `query_panels_with_room_nums`): 查询房间与面板的映射。SQL查询 `select value [in, room_num] from room_relate group by in, room_num`，返回Vec<(RefnoEnum, String)>。

#### 房间房间关键字和名称匹配

- `src/fast_model/room_model.rs` (lines 791-799): 房间名称匹配函数。支持项目特性条件编译：
  - match_room_name_hd: 正则表达式匹配 `^[A-Z]\d{3}$` (字母+3位数字)
  - match_room_name_hh: 接受所有房间名称

- `src/fast_model/room_model.rs` (lines 254-301, `build_room_panel_query_sql`): 房间面板查询SQL构建。支持项目特性条件编译(project_hd/project_hh)。基础查询从FRMW查询SBFR->PANE层级，通过NAME过滤和1-2层children递归。

---

## Report (The Answers)

### result

#### 1. 房间数据在SurrealDB和SQLite中的存储结构

**SurrealDB 存储结构**：
- **房间面板关系表**：`room_panel_relate` - 存储房间与面板的关联
- **房间构件关系表**：`room_relate` - 存储面板与构件的房间关系
  - 关系格式：`panel_refno->room_relate->component_refno`
  - 属性字段：
    - `room_num`: VARCHAR - 房间号字符串
    - `confidence`: FLOAT - 置信度（默认0.9）
    - `created_at`: TIMESTAMP - 创建时间戳

**MySQL 存储结构**：
- **房间码表**：`ROOM_CODE`
  - REFNO (BIGINT) - 房间参考号
  - ROOM_NAME (VARCHAR(50)) - 房间号/房间名称

**SQLite 存储结构**：
- **ref_bbox 表**：主AABB缓存
  - refno (INTEGER PRIMARY KEY)
  - data (BLOB) - 二进制序列化的StoredRStarBBox
- **versioned_ref_bbox 表**：版本化AABB
  - refno_key (TEXT) - RefnoEnum字符串形式
  - session (INTEGER) - 会话号
  - data (BLOB) - VersionedStoredAabb二进制数据
  - PRIMARY KEY (refno_key, session)
- **aabb_index 虚表**：3D R*-tree空间索引
  - id (INTEGER) - 主键/refno
  - min_x, max_x, min_y, max_y, min_z, max_z (FLOAT) - 三维AABB范围

#### 2. 房间相关的核心数据结构定义

**RoomBuildStats** - 房间构建统计结构：
```
pub struct RoomBuildStats {
    pub total_rooms: usize,           // 处理的房间总数
    pub total_panels: usize,          // 处理的面板总数
    pub total_components: usize,      // 计算的构件总数
    pub build_time_ms: u64,           // 构建耗时（毫秒）
    pub cache_hit_rate: f32,          // 缓存命中率（0-1）
    pub memory_usage_mb: f32,         // 内存使用量（MB）
}
```

**RoomComputeOptions** - 房间计算配置：
```
struct RoomComputeOptions {
    inside_tol: f32,      // 点包含容差（默认0.1）
    concurrency: usize,   // 并发度（默认4）
}
```

**房间面板映射** - Vec<(RefnoEnum, String, Vec<RefnoEnum>)>三元组：
- 第一个元素：房间的RefnoEnum（房间参考号）
- 第二个元素：房间号字符串（如"A123"或"-RM"标识的房间号）
- 第三个元素：属于该房间的面板RefnoEnum列表

**缓存数据结构**：
- ENHANCED_GEOMETRY_CACHE: DashMap<String, Arc<PlantMesh>> - 并发PlantMesh缓存，键为"{geo_hash}_L0"
- ENHANCED_TRIMESH_CACHE: DashMap<String, Arc<TriMesh>> - 并发TriMesh缓存

**AABB 相关结构**：
- StoredAabb：mins/maxs 都是[f32;3]，表示AABB的最小和最大点
- StoredRStarBBox：包含AABB、refno(u64)和noun(String)三个字段
- VersionedStoredAabb：包含refno、session、mins、maxs、created_at、updated_at

#### 3. 房间与其他实体的关系映射

**房间层级结构**（SurrealDB中的对象层级）：
```
FRMW (Frame - 房间框架)
  └── SBFR (SubFrame - 子框架)
      └── PANE (Panel - 面板)
          ├── GeomInst (几何实例)
          └── Components (构件)
```

**关系映射流程**：
1. **房间 → 面板关系**：通过room_panel_relate关系存储
   - 查询方式：通过FRMW的OWNER字段递归查询SBFR，再查询PANE
   - 支持项目特定条件编译优化不同的查询策略

2. **面板 → 构件关系**：通过room_relate关系存储
   - 每条room_relate关系存储一个面板到构件的关系
   - 包含room_num属性，表示该构件所在的房间号

3. **查询优化**：
   - 粗算阶段：使用SQLite RTree空间索引查询AABB相交的候选构件
   - 细算阶段：使用关键点包含测试精确判定（50%投票策略）

**AABB与参考号的关系**：
- 每个参考号（refno）的AABB存储在ref_bbox表中
- AABB通过SQLite RTree建立空间索引，支持快速范围查询
- 版本化存储支持多个会话号（sesno）的历史追踪

#### 4. AABB 边界框的计算和缓存机制

**AABB 计算流程**：
1. **几何体AABB计算**：从房间面板的GeomInstQuery获取world_aabb
2. **关键点提取**：从AABB提取27个关键点（8个顶点、1个中心、6个面中心、12条边中点）
3. **精确判定**：使用parry3d TriMesh.project_point()计算点到面板几何的距离
   - 距离判定：distance_sq <= tolerance_sq (tolerance=0.1)
   - 投票策略：50%以上的关键点在面板内则判定为包含

**缓存机制**：
- **两级缓存**：
  1. PlantMesh缓存（ENHANCED_GEOMETRY_CACHE）：DashMap存储Arc<PlantMesh>
  2. TriMesh缓存（ENHANCED_TRIMESH_CACHE）：DashMap存储Arc<TriMesh>（L0 LOD、未应用变换）

- **缓存键策略**："{geo_hash}_L0" 格式，使用几何体哈希和LOD级别标识

- **缓存管理**：
  - 并发访问：使用DashMap支持多线程并发读写
  - 自动清理：超过2000条目时移除前50%的条目
  - 统计指标：CacheMetrics追踪命中/未命中数，计算命中率

- **缓存命中率**：通过CacheMetrics统计，plant_hits和trimesh_hits的总和除以总访问数

**SQLite R*-tree 空间索引**：
- **虚表定义**：`CREATE VIRTUAL TABLE aabb_index USING rtree(id, min_x, max_x, min_y, max_y, min_z, max_z)`
- **查询操作**：
  - INSERT：插入AABB数据到RTree虚表
  - SELECT：支持多维范围查询和相交判定
- **查询参数**：基于6参数AABB(min_x, max_x, min_y, max_y, min_z, max_z)的相交查询
- **查询优化**：sqlite_query_intersect()函数使用硬件加速的范围查询

#### 5. 房间计算的并发和增量更新

**并发处理**：
- 使用futures::stream::StreamExt和buffer_unordered()控制并发度（默认4）
- 对每个房间的面板进行异步并发处理
- 使用tokio::task::spawn_blocking进行CPU密集的空间索引查询

**增量更新支持**：
- update_room_relations_incremental()函数支持部分refnos的增量更新
- 流程：查询受影响面板 → 删除旧关系 → 重新计算新关系
- RoomRelationUpdateResult返回：affected_rooms、updated_panels、updated_components数量

---

## conclusions

- 房间数据跨越三个数据库系统：SurrealDB存储关系和元数据，MySQL存储房间号映射，SQLite存储空间索引和AABB缓存
- 房间与构件的关系通过room_relate关系表存储，包含房间号和置信度两个属性
- AABB缓存采用两级设计（PlantMesh + TriMesh），通过DashMap实现并发访问，缓存键使用"{geo_hash}_L0"格式
- 房间构件计算采用两阶段算法：粗算阶段使用SQLite R*-tree查询候选，细算阶段使用50%投票的关键点包含测试
- 房间名称匹配支持项目特定条件编译，HD项目要求"[A-Z]\d{3}"格式，HH项目接受所有格式
- 版本化AABB存储支持会话号和时间戳追踪，支持多版本历史查询
- 房间计算支持增量更新，可通过指定受影响的refnos部分更新相关房间关系

---

## relations

- `src/fast_model/room_model.rs` (`build_room_relations`) 调用 `build_room_panels_relate()` 查询房间面板映射，调用 `compute_room_relations()` 并发处理
- `src/fast_model/room_model.rs` (`compute_room_relations`) 调用 `process_panel_for_room()` 处理每个面板
- `src/fast_model/room_model.rs` (`process_panel_for_room`) 调用 `cal_room_refnos()` 计算房间内构件，调用 `save_room_relate()` 保存关系
- `src/fast_model/room_model.rs` (`cal_room_refnos`) 调用 `load_geometry_with_enhanced_cache()` 加载面板几何，调用 aios_core 的 `query_overlap()` 进行空间索引查询
- `src/fast_model/room_model.rs` (`cal_room_refnos`) 调用 `extract_geom_key_points()` 和 `is_geom_in_panel()` 进行细算判定
- `src/fast_model/aabb_cache.rs` (`sqlite_query_intersect`) 使用SQLite R*-tree虚表查询相交的AABB
- `src/fast_model/aabb_cache.rs` (`put_ref_bbox_versioned`) 存储版本化AABB数据到SQLite versioned_ref_bbox表
- `src/api/room_code.rs` 查询MySQL中的ROOM_CODE表获取房间号
- `src/tables.rs` (`gen_create_room_code_table_sql`) 定义MySQL ROOM_CODE表结构
