<!-- 房间计算核心算法和空间索引机制深度调查报告 -->

### Code Sections (The Evidence)

#### 1. R*-tree 空间索引的核心实现

- `src/spatial_index.rs` (SqliteSpatialIndex): SQLite 基于虚表的 R*-tree 实现
  - 虚表定义（行180-182）：`CREATE VIRTUAL TABLE aabb_index USING rtree(id, min_x, max_x, min_y, max_y, min_z, max_z)`
  - 支持 6 维空间索引（3D AABB 的两个端点）

- `src/fast_model/aabb_cache.rs` (AabbCache::sqlite_query_intersect): R*-tree 相交查询（行320-342）
  - SQL 查询模式：`SELECT id FROM aabb_index WHERE max_x >= ?1 AND min_x <= ?2 AND max_y >= ?3 AND min_y <= ?4 AND max_z >= ?5 AND min_z <= ?6`
  - 参数：查询 AABB 的 min_x, max_x, min_y, max_y, min_z, max_z（共 6 个浮点数）
  - 返回：与查询范围重叠的所有 RefU64 对象列表

- `src/spatial_index.rs` (SqliteSpatialIndex::query_intersect): 基础相交查询（行281-305）
  - 接收查询 AABB，转换为 f64 精度
  - 调用 `query_map()` 映射 SQL 结果为 RefU64 列表
  - 支持原子事务操作

#### 2. 房间空间查询的四阶段处理

- `src/fast_model/room_model.rs` (cal_room_refnos): 房间构件计算主函数（行444-564）

  **阶段 1：面板几何加载（行452-481）**
  - 查询面板的 GeomInstQuery 结构：`aios_core::query_insts(&[panel_refno], true).await`
  - 使用 `load_geometry_with_enhanced_cache()` 加载 L0 LOD 网格（最低精度以节省内存）
  - 构建 parry3d TriMesh：`mesh.get_tri_mesh_with_flag(world_trans, TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES)`

  **阶段 2：粗算 - SQLite RTree 空间查询（行483-517）**
  - 使用 `aios_core::spatial::sqlite::query_overlap(&panel_aabb, None, Some(1000), &exclude_list)`
  - 参数：面板的 AABB、无额外过滤、限制 1000 个候选、排除集合
  - 返回与面板 AABB 重叠的候选构件列表（已排除面板本身和其他面板）

  **阶段 3：细算 - 关键点精确检测（行519-547）**
  - 对每个候选构件调用 `aios_core::query_insts()` 获取其几何实例
  - 调用 `extract_geom_key_points()` 提取候选构件的关键点
  - 调用 `is_geom_in_panel()` 进行精确几何检测（点包含测试）
  - 使用投票策略：50%以上的关键点在面板内判定为属于该房间

  **阶段 4：关系保存（行428）**
  - 调用 `save_room_relate()` 保存 room_relate 关系到 SurrealDB

#### 3. AABB 关键点提取和精确检测算法

- `src/fast_model/room_model.rs` (extract_aabb_key_points): AABB 关键点提取（行633-675）
  - 提取 27 个关键点：
    - 8 个顶点：`aabb.vertices()`（立方体顶点）
    - 1 个中心点：`aabb.center()`
    - 6 个面中心点：每个面中心（min_x 面、max_x 面等）
    - 12 个边中点：每条边的中点（底面 4 条边、顶面 4 条边、竖直 4 条边）
  - 使用 parry3d Point<Real> 表示点坐标
  - 总计容量预分配：Vec::with_capacity(27)

- `src/fast_model/room_model.rs` (extract_geom_key_points): 几何体关键点提取（行678-688）
  - 对每个 GeomInstQuery 提取其 world_aabb 的 27 个关键点
  - 多个几何实例的关键点合并为一个列表
  - 返回所有构件的关键点集合

- `src/fast_model/room_model.rs` (is_geom_in_panel): 点包含判定（行692-713）
  - 使用 parry3d TriMesh 的 `project_point()` 方法进行点投影
  - 判定条件：`projection.is_inside || distance_sq <= tolerance_sq`
  - 容差参数：默认 0.1（function 参数）
  - 阈值投票：计算有多少关键点在面板内
  - 返回条件：`points_inside >= (total_points * 0.5)`（50%阈值）

#### 4. 房间关系保存的批量处理机制

- `src/fast_model/room_model.rs` (save_room_relate): 房间关系保存函数（行716-749）
  - 输入：panel_refno（面板参考号）、within_refnos（HashSet 形式的房间内构件）、room_num（房间号字符串）
  - 核心逻辑：
    1. 遍历 within_refnos 中的每个构件
    2. 为每个构件生成唯一的 relation_id：`format!("{}_{}", panel_refno, refno)`
    3. 构造 SurrealDB RELATE 语句：
       ```sql
       relate panel_refno->room_relate:relation_id->refno set room_num='...', confidence=0.9, created_at=time::now();
       ```
    4. 所有 SQL 语句用 `\n` 拼接成批量语句
    5. 通过 `SUL_DB.query(&batch_sql)` 一次性执行所有语句
  - 输出：返回 anyhow::Result<()>

#### 5. 并发处理策略

- `src/fast_model/room_model.rs` (compute_room_relations): 并发处理入口（行200-251）
  - 使用 futures::stream 的流处理方式
  - `.buffer_unordered(options.concurrency.max(1))` 控制并发度
  - 默认并发度：`default_room_concurrency()` 从环境变量 `ROOM_RELATION_CONCURRENCY` 读取，默认为 4
  - 流程：为每个房间-面板组合创建异步任务，批量处理多个面板

- `src/fast_model/room_model.rs` (RoomComputeOptions): 房间计算选项（行55-68）
  - inside_tol: 点包含的容差（默认 0.1）
  - concurrency: 并发度（默认 4）

#### 6. 增强的几何缓存机制

- `src/fast_model/room_model.rs` (ENHANCED_GEOMETRY_CACHE): 全局几何网格缓存（行131-132）
  - 类型：`tokio::sync::OnceCell<DashMap<String, Arc<PlantMesh>>>`
  - 使用 OnceCell 确保单例初始化
  - DashMap 支持无锁并发访问

- `src/fast_model/room_model.rs` (load_geometry_with_enhanced_cache): 缓存加载函数（行567-613）
  - 缓存键：`format!("{}_L0", geo_hash)`（使用 L0 LOD 标识）
  - 缓存命中流程：
    1. 从缓存获取 PlantMesh
    2. 调用 `get_tri_mesh_with_flag(world_trans * inst.transform, TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES)`
    3. 记录命中统计：`CACHE_METRICS.record_plant_hit()`
  - 缓存未命中流程：
    1. 使用 LOD 路径检测：`build_mesh_path(geo_hash, "L0")`
    2. 异步加载：`tokio::task::spawn_blocking(|| PlantMesh::des_mesh_file(&file_path))`
    3. 构建 TriMesh
    4. 插入缓存：`cache.insert(cache_key, Arc::new(mesh))`
    5. 记录未命中统计：`CACHE_METRICS.record_plant_miss()`
  - 缓存清理：当缓存大小超过 2000 时，移除一半的条目

- `src/fast_model/room_model.rs` (CacheMetrics): 缓存统计结构（行78-127）
  - 字段：plant_hits、plant_misses、trimesh_hits、trimesh_misses（都是 AtomicU64）
  - 命中率计算：`hits / (hits + misses)`
  - 原子操作：使用 Relaxed 内存顺序（性能优先）

#### 7. SQLite 连接优化配置

- `src/spatial_index.rs` (SqliteSpatialIndex::configure_connection): 连接配置（行160-166）
  - WAL 模式：`pragma journal_mode = WAL`（写入时提前日志）
  - 同步级别：`pragma synchronous = NORMAL`（平衡性能和安全）
  - 缓存大小：`pragma cache_size = 10000`（10000 页缓存）
  - 临时存储：`pragma temp_store = MEMORY`（内存临时表）

- `src/spatial_index.rs` (SqliteSpatialIndex::insert_aabb): 原子 AABB 写入（行198-225）
  - 使用事务：`conn.unchecked_transaction()`
  - 同时更新：items 表和 aabb_index 虚表
  - INSERT OR REPLACE 策略确保幂等性

#### 8. 高级空间查询功能

- `src/spatial_index.rs` (SqliteSpatialIndex::query_by_overlap): 统一查询函数（行335-463）
  - 支持两种查询模式：
    - 相交查询：`max_x >= ? AND min_x <= ? AND ...`（6 个范围条件）
    - 包含查询：`min_x >= ? AND max_x <= ? AND ...`（6 个范围条件）
  - 支持容差：`tol = opts.tolerance.max(0.0)`，应用于 AABB 扩展
  - 支持类型过滤：JOIN items 表进行 noun 过滤
  - 支持排除列表：`NOT IN` 子查询
  - 支持排序：按 ID 升序/降序或按距离排序
  - 支持限制：LIMIT 子句

- `src/spatial_index.rs` (SqliteSpatialIndex::query_knn_point): K-近邻查询（行467-508）
  - 自适应搜索半径：初始值 1.0，每次迭代扩大 2 倍
  - 迭代最大次数：10 次
  - 过采样倍数：`k * 8`（获取更多候选，然后过滤）
  - 距离计算：`distance_point_aabb(point, bb)`
  - 排序并返回最近的 k 个

- `src/spatial_index.rs` (distance_point_aabb): 点到 AABB 的最短距离（行750-773）
  - 计算每个轴的距离分量：
    - 如果点在 AABB 内该轴，距离 = 0
    - 如果点小于最小值，距离 = min - point
    - 如果点大于最大值，距离 = point - max
  - 返回 3D 欧几里得距离：`sqrt(dx² + dy² + dz²)`

#### 9. 房间面板映射构建流程

- `src/fast_model/room_model.rs` (build_room_panels_relate): 房间面板关系构建（行304-315）
  - 根据编译特性选择不同的 SQL 查询策略：
    - project_hd：FRMW -> SBFR -> PANE（三层递归）
    - project_hh：SBFR -> PANE（两层递归）
    - 默认：FRMW -> SBFR -> PANE
  - 使用房间关键词进行过滤：`'keyword' in NAME`

- `src/fast_model/room_model.rs` (build_room_panel_query_sql): SQL 生成函数（行254-301）
  - 使用 SurrealDB 的 OWNER 字段递归查询
  - 使用 array::last() 提取房间号（按 '-' 分割）
  - 返回三元组：[room_id, room_num, panel_refnos_array]

- `src/fast_model/room_model.rs` (create_room_panel_relations_batch): 批量创建房间面板关系（行385-405）
  - 为每个房间-面板组合生成 RELATE 语句
  - 使用 to_pe_key() 转换为 PE (PDMS Expression) 格式
  - 批量执行：`batch_sql.join("\n")`

#### 10. 数据版本化支持

- `src/fast_model/aabb_cache.rs` (AabbCache::put_ref_bbox_versioned): 版本化存储（行493-528）
  - 表结构：versioned_ref_bbox (refno_key TEXT, session INTEGER, data BLOB)
  - RefnoEnum 转换：session=0 表示当前版本，session>0 表示历史版本
  - 时间戳记录：created_at 和 updated_at（UNIX 秒级）
  - 主键：(refno_key, session) 组合唯一

- `src/fast_model/aabb_cache.rs` (AabbCache::get_ref_bbox_history): 历史版本查询（行552-588）
  - SQL：`SELECT session, data FROM versioned_ref_bbox WHERE refno_key LIKE ?1 ORDER BY session`
  - 前缀匹配查询 refno 的所有 session 版本
  - 返回：Vec<(u32, RStarBoundingBox)>，按 session 升序排列

#### 11. 统计监控信息

- `src/fast_model/room_model.rs` (RoomBuildStats): 房间构建统计（行45-53）
  - total_rooms：处理的房间数
  - total_panels：处理的面板数
  - total_components：计算出的构件数
  - build_time_ms：总耗时（毫秒）
  - cache_hit_rate：缓存命中率（0.0-1.0）
  - memory_usage_mb：预估内存占用（MB）

- `src/fast_model/room_model.rs` (estimate_memory_usage): 内存估算（行776-788）
  - 假设每个缓存项平均 0.5MB
  - 公式：`cache_size = cache.len() as f32 * 0.5`

---

### Report (The Answers)

#### result

**1. 房间空间查询的实现原理**

房间空间查询采用**四阶段分层处理**机制（cal_room_refnos 函数，行 444-564）：

1. **面板几何加载**：查询面板的 GeomInstQuery，使用 L0 LOD 网格减少内存占用，构建 parry3d TriMesh 用于后续点检测

2. **粗算阶段**：使用 SQLite RTree 空间索引快速过滤候选构件
   - 查询函数：`aios_core::spatial::sqlite::query_overlap(&panel_aabb, None, Some(1000), &exclude_list)`
   - 返回与面板 AABB 重叠的最多 1000 个候选构件

3. **细算阶段**：对每个候选构件进行精确几何检测
   - 提取 27 个关键点（8 个顶点 + 中心 + 6 个面中心 + 12 个边中点）
   - 使用 parry3d TriMesh.project_point() 进行点投影
   - 应用 50% 投票策略：超过一半关键点在面板内则判定为属于该房间

4. **关系保存**：批量生成 SurrealDB RELATE 语句，使用单次数据库往返提交所有关系

**2. R*-tree 空间索引的使用方式**

- **虚表定义**（sqlite_index.rs 行 180-182）：`CREATE VIRTUAL TABLE aabb_index USING rtree(id, min_x, max_x, min_y, max_y, min_z, max_z)`
  - 6 维索引对应 3D AABB 的两个端点（min_x/max_x, min_y/max_y, min_z/max_z）

- **查询方式**（aabb_cache.rs 行 320-342）：
  ```sql
  SELECT id FROM aabb_index
  WHERE max_x >= ?1 AND min_x <= ?2
    AND max_y >= ?3 AND min_y <= ?4
    AND max_z >= ?5 AND min_z <= ?6
  ```
  - 6 个条件对应查询 AABB 的两个端点
  - SQLite RTree 自动优化搜索路径（B+ 树结构）

- **数据维护**（aabb_cache.rs 行 452-479）：
  - put_ref_bbox() 同时更新 ref_bbox 表（存储完整数据）和 aabb_index 虚表（存储索引坐标）
  - 使用 INSERT OR REPLACE 策略保证幂等性
  - 支持版本化：versioned_ref_bbox 表存储历史版本

**3. AABB 包围盒相交测试算法**

采用**两步相交判定**：

1. **SQLite RTree 快速检测**（SQL 层级）：
   - 使用 6 维范围条件判定 AABB 重叠
   - 条件逻辑：`max_x >= query_min_x AND min_x <= query_max_x AND ...`（三个维度相同）
   - 时间复杂度：O(log N)，N 为索引中的 AABB 数量

2. **parry3d TriMesh 精确检测**（几何层级）：
   - 提取 27 个关键点覆盖 AABB 整体形状
   - 对每个点使用 `TriMesh.project_point()` 计算投影
   - 判定逻辑：`distance <= tolerance` 或 `is_inside == true`
   - 容差默认：0.1（可配置）

**4. 房间拓扑关系的构建流程**

1. **查询房间-面板映射**（build_room_panels_relate，行 304-315）：
   - 根据房间关键词查询 SurrealDB
   - 支持多种项目特性：project_hd（三层递归）、project_hh（两层递归）
   - SQL 语法：使用 OWNER 字段递归查询 FRMW -> SBFR -> PANE

2. **计算房间内构件**（cal_room_refnos，行 444-564）：
   - 对每个面板执行四阶段查询
   - 返回属于该房间的构件 HashSet<RefnoEnum>

3. **保存房间关系**（save_room_relate，行 716-749）：
   - 为每个构件生成唯一的 relation_id
   - 构造 RELATE 语句：`relate panel_refno->room_relate:relation_id->refno set room_num='...', confidence=0.9, created_at=time::now();`
   - 批量执行：单次数据库往返提交所有关系

4. **并发处理**（compute_room_relations，行 200-251）：
   - 使用 `buffer_unordered(concurrency)` 控制并发度（默认 4）
   - 为每个房间-面板组合创建异步任务
   - 统计处理时间、缓存命中率、内存占用

**5. 性能优化策略**

- **批量处理**：
  - 房间关系保存：多个 RELATE 语句用 `\n` 拼接，单次提交
  - SQL 查询：`Some(1000)` 限制候选数量（避免过度计算）

- **缓存机制**：
  - PlantMesh 缓存：使用 DashMap<String, Arc<PlantMesh>> 存储 L0 LOD 网格
  - 缓存键：`"{geo_hash}_L0"`（基于 geohash 和 LOD 级别）
  - 缓存清理：大小超过 2000 时自动清理一半条目
  - 统计指标：AtomicU64 原子计数器追踪命中率（Relaxed 内存顺序）

- **几何优化**：
  - 使用 L0 LOD 而非完整网格（减少 I/O 和内存）
  - TriMesh 标志：ORIENTED | MERGE_DUPLICATE_VERTICES（去重并加快查询）
  - 关键点采样（27 个点）而非完整顶点检查

- **并发优化**：
  - 异步任务：`tokio::spawn_blocking` 用于 CPU 密集的 TriMesh 构建和点投影
  - 无锁集合：DashMap 支持并发读写而无需全局锁
  - 流处理：futures::stream 的 `buffer_unordered()` 动态调度任务

- **数据库优化**：
  - WAL 模式：提前写日志，提高并发性
  - 缓存大小：10000 页缓存加速查询
  - 临时表：使用内存而非磁盘
  - 事务：unchecked_transaction 降低开销

---

#### conclusions

**关键事实总结**

1. **四阶段分层查询设计**
   - 粗算：SQLite RTree（单个 SQL 查询，O(log N) 复杂度）
   - 细算：27 点投票（最多 1000 × 27 = 27000 个点投影）
   - 两级过滤器减少计算量（从所有构件 -> 候选 -> 最终结果）

2. **R*-tree 作为核心空间加速结构**
   - 6 维虚表支持 3D AABB 快速查询
   - 相交判定使用 6 个范围条件（每维两个）
   - 自动平衡 B+ 树保证对数性能

3. **关键点投票策略的数学基础**
   - 27 个点采样覆盖 AABB：顶点（极值点）+ 面中心（面代表）+ 边中点（边代表）
   - 50% 阈值：鲁棒性与精准性的权衡
   - parry3d 点投影：支持容差（默认 0.1）处理边界情况

4. **批量 SurrealDB 操作的设计**
   - 多个 RELATE 语句用换行符分隔
   - 单次网络往返提交所有关系（对比逐个提交）
   - relation_id 确保关系唯一性和可追踪性

5. **DashMap 和 Arc 的无锁缓存设计**
   - OnceCell 确保单例初始化
   - Arc<PlantMesh> 支持零拷贝共享
   - Relaxed 原子操作性能优先
   - 缓存大小 2000 个条目，平均单个 0.5MB

6. **版本化存储支持增量更新**
   - RefnoSesno 绑定 refno 与 session
   - versioned_ref_bbox 表存储历史快照
   - (refno_key, session) 复合主键保证唯一性
   - 支持时间旅行查询

7. **并发度控制的自适应设计**
   - 环境变量 ROOM_RELATION_CONCURRENCY 允许动态调整（默认 4）
   - buffer_unordered() 而非 buffer_unordered(fixed_size) 提供更好的负载均衡
   - AtomicU64 原子计数无需锁就能统计命中率

---

#### relations

**代码依赖关系链**

1. **房间计算主流程链**
   - build_room_relations() (room_model.rs:160)
   - → build_room_panels_relate() (room_model.rs:304) 查询房间-面板映射
   - → compute_room_relations() (room_model.rs:200) 并发处理
   - → process_panel_for_room() (room_model.rs:408) 单个面板处理
   - → cal_room_refnos() (room_model.rs:444) 四阶段房间构件计算
   - → aios_core::spatial::sqlite::query_overlap() 粗算查询
   - → is_geom_in_panel() (room_model.rs:692) 细算点检测
   - → save_room_relate() (room_model.rs:716) 关系保存

2. **空间索引查询链**
   - cal_room_refnos() (room_model.rs:444)
   - → tokio::task::spawn_blocking() + aios_core::spatial::sqlite::query_overlap()
   - → [外部库调用] aios_core 中的 sqlite 模块
   - → [委托回] src/spatial_index.rs SqliteSpatialIndex::query_intersect() 或 aabb_cache.rs sqlite_query_intersect()
   - → SQLite RTree 虚表查询

3. **几何缓存加载链**
   - cal_room_refnos() (room_model.rs:444)
   - → load_geometry_with_enhanced_cache() (room_model.rs:567)
   - → get_enhanced_geometry_cache() (room_model.rs:140) 获取全局缓存单例
   - → PlantMesh::des_mesh_file() [异步阻塞任务] 磁盘加载 L0 LOD 网格
   - → mesh.get_tri_mesh_with_flag() 构建 parry3d TriMesh
   - → DashMap.insert() 存储到缓存

4. **点精确检测链**
   - is_geom_in_panel() (room_model.rs:692)
   - → extract_geom_key_points() (room_model.rs:678) 提取候选构件的关键点
   - → extract_aabb_key_points() (room_model.rs:633) 从 AABB 展开 27 个点
   - → panel_tri_mesh.project_point() [parry3d] 点投影并判定
   - → 阈值投票：points_inside >= (total_points * 0.5)

5. **关系保存批量处理链**
   - process_panel_for_room() (room_model.rs:408)
   - → save_room_relate() (room_model.rs:716)
   - → for refno in within_refnos: 循环构建 RELATE 语句
   - → format!("relate {}->room_relate:{}->{}  set room_num='{}', confidence=0.9, created_at=time::now();") SQL 语句生成
   - → batch_sql.join("\n") 批量拼接
   - → SUL_DB.query(&batch_sql) 提交执行

6. **并发管理链**
   - compute_room_relations() (room_model.rs:200)
   - → stream::iter() 创建流
   - → .map() 将房间-面板转换为异步任务
   - → .buffer_unordered(options.concurrency) 限制并发度
   - → .collect() 等待所有任务完成
   - → RoomBuildStats 统计汇总

7. **缓存统计链**
   - CACHE_METRICS (room_model.rs:138) 全局静态变量
   - → load_geometry_with_enhanced_cache() (room_model.rs:567)
   - → CACHE_METRICS.record_plant_hit() 或 record_plant_miss()
   - → CACHE_METRICS.hit_rate() 计算 (hits) / (hits + misses)
   - → RoomBuildStats.cache_hit_rate 报告结果

8. **数据库持久化链**
   - AabbCache::put_ref_bbox() (aabb_cache.rs:452)
   - → conn.execute() 插入 ref_bbox 表
   - → conn.execute() 同时插入 aabb_index 虚表
   - → INSERT OR REPLACE 策略保证幂等
   - → 支持版本化：put_ref_bbox_versioned() (aabb_cache.rs:493)

9. **SQLite 连接优化链**
   - SqliteSpatialIndex::get_connection() (spatial_index.rs:153)
   - → Connection::open() 打开数据库
   - → configure_connection() (spatial_index.rs:160) 配置连接
   - → pragma journal_mode = WAL
   - → pragma synchronous = NORMAL
   - → pragma cache_size = 10000
   - → pragma temp_store = MEMORY

10. **高级查询能力链**
    - SqliteSpatialIndex::query_by_overlap() (spatial_index.rs:335)
    - → 支持容差扩展：tol = opts.tolerance
    - → 支持类型过滤：JOIN items ON items.id = aabb_index.id
    - → 支持排除列表：NOT IN 子查询
    - → 支持排序：ORDER BY id 或按距离排序
    - → 支持限制：LIMIT
    - → distance_point_aabb() (spatial_index.rs:750) 距离计算

---

### Implementation Evidence (Algorithm Details)

**关键算法详解**

#### AABB 相交判定（SQL 层级）

相交判定的 SQL 条件基于 **分离轴定理 (Separating Axis Theorem, SAT)**：

两个 AABB 相交当且仅当在所有三个轴上都有重叠。在每个轴上，两个 AABB 的投影重叠条件为：
```
aabb1_min <= aabb2_max AND aabb1_max >= aabb2_min
```

SQL 查询（6 个条件）：
```sql
WHERE max_x >= ?1 AND min_x <= ?2    -- X 轴重叠
  AND max_y >= ?3 AND min_y <= ?4    -- Y 轴重叠
  AND max_z >= ?5 AND min_z <= ?6    -- Z 轴重叠
```

#### 27 点关键点采样的数学意义

AABB 的 27 个关键点分布：
- **8 个顶点**：AABB 的极值点，代表空间角落
- **1 个中心点**：质心，代表整体位置
- **6 个面中心点**：每个面的中点，代表面的位置
- **12 个边中点**：每条边的中点，代表边的位置

这个采样方案覆盖了：
- 极值情况（顶点）
- 中等情况（面中心、边中点）
- 整体趋势（中心）

采样密度足以检测构件与面板的主要接触点。

#### 50% 投票策略的鲁棒性

给定 n 个关键点，如果有 k 个点在面板内：
- k >= n/2：判定为"构件在房间内"（多数投票）
- k < n/2：判定为"构件在房间外"

这个策略的优势：
- **鲁棒性**：单个错误判定（如数值误差导致的边界点误判）不会改变结果
- **容错率**：最多可容忍 50% 的错误点
- **精准性**：50% 的浮点数（对于 27 个点，约 13.5）保证了充分的区分度

---

### Testing & Validation Points

**核心算法需要验证的关键点**

1. **RTree 相交查询的正确性**
   - 验证：query_overlap() 返回的候选数量是否正确
   - 验证：是否包含所有与 AABB 重叠的构件
   - 验证：是否排除了不重叠的构件

2. **27 点关键点的完整性**
   - 验证：extract_aabb_key_points() 返回 27 个不重复的点
   - 验证：点的分布是否均匀覆盖 AABB

3. **50% 投票策略的决策边界**
   - 验证：恰好 50% 点在面板内时的判定结果
   - 验证：49% vs 51% 的区分效果

4. **缓存命中率与性能**
   - 验证：DashMap 缓存的实际命中率
   - 验证：L0 LOD 网格的精度是否足以进行点投影

5. **并发度的影响**
   - 验证：不同 ROOM_RELATION_CONCURRENCY 值下的吞吐量
   - 验证：是否存在内存峰值或死锁

6. **批量保存的原子性**
   - 验证：多个 RELATE 语句的一致性
   - 验证：部分失败时的恢复机制

