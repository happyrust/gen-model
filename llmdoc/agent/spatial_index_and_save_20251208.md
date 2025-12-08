<!-- 空间索引和关系保存机制深度调查报告 -->

### Code Sections (The Evidence)

#### SQLite RTree 空间索引的数据结构和查询

- `src/fast_model/aabb_cache.rs` (AabbCache): 主AABB缓存管理类，包含SQLite表定义和RTree操作
  - 虚表定义（行235-238）：`CREATE VIRTUAL TABLE aabb_index USING rtree(id, min_x, max_x, min_y, max_y, min_z, max_z)`
  - 支持6维R-Tree索引（3D AABB的两个端点）

- `src/fast_model/aabb_cache.rs` (sqlite_query_intersect): 空间相交查询函数（行320-342）
  - SQL查询模式：基于AABB范围的6参数相交判定
  - 参数：查询AABB的min_x, max_x, min_y, max_y, min_z, max_z
  - 返回：与查询范围重叠的所有RefU64对象列表

- `src/fast_model/aabb_cache.rs` (sqlite_rebuild_from_internal): RTree重建函数（行280-317）
  - 从ref_bbox表读取所有数据，插入RTree虚表
  - 数据序列化格式：StoredRStarBBox（包含AABB + refno + noun）
  - bincode二进制序列化/反序列化

- `src/fast_model/aabb_cache.rs` (sqlite_get_aabb): 点查询函数（行345-371）
  - 按RefU64查询对应的AABB值
  - 返回6参数形式的AABB（min_x,max_x,min_y,max_y,min_z,max_z）

#### 房间关系保存流程

- `src/fast_model/room_model.rs` (save_room_relate): 房间关系保存主函数（行678-711）
  - 输入：panel_refno (面板参考号)、within_refnos (房间内的构件集合)、room_num (房间号字符串)
  - 核心逻辑：为每个构件生成RELATE语句，格式为 `relate panel_refno->room_relate->refno set room_num='...', confidence=0.9`
  - 批量执行：将所有SQL语句用换行符拼接，一次性提交到SurrealDB
  - 特点：使用panel_refno和refno的PE_KEY表示，生成唯一的relation_id

- `src/fast_model/room_model.rs` (process_panel_for_room): 面板处理包装函数（行376-408）
  - 调用cal_room_refnos()计算房间内构件
  - 调用save_room_relate()保存关系
  - 错误处理：返回成功保存的构件数量

#### 空间索引查询的多阶段处理

- `src/fast_model/room_model.rs` (cal_room_refnos): 房间构件计算主函数（行412-526）
  - 四个阶段的实现细节：

  **阶段1：加载面板几何（行420-443）**
  - 查询面板的GeomInstQuery结构
  - 使用load_geometry_with_enhanced_cache()加载L0 LOD网格
  - 构建parry3d TriMesh用于点包含测试

  **阶段2：粗算-空间索引查询（行445-479）**
  - 调用aios_core::spatial::sqlite::query_overlap()
  - 参数：panel_aabb, None, Some(1000), &exclude_list
  - 返回与面板AABB重叠的候选构件列表（最多1000个）
  - 排除面板本身和其他面板

  **阶段3：细算-关键点检测（行481-515）**
  - 对每个候选构件调用extract_geom_key_points()
  - 使用is_geom_in_panel()进行精确几何检测
  - 投票策略：50%以上的关键点在面板内判定为属于该房间

  **阶段4：保存关系（行396）**
  - 调用save_room_relate()保存room_relate关系到SurrealDB

#### 关键点提取和精确检测算法

- `src/fast_model/room_model.rs` (extract_aabb_key_points): AABB关键点提取（行595-637）
  - 提取27个关键点：8个顶点 + 1个中心 + 6个面中心 + 12个边中点
  - 使用parry3d::math::Point<Real>表示点
  - 顶点顺序遵循标准立方体顶点编号

- `src/fast_model/room_model.rs` (extract_geom_key_points): 几何体关键点提取（行640-650）
  - 对每个GeomInstQuery提取其world_aabb的27个关键点
  - 多个几何实例的关键点合并为一个列表

- `src/fast_model/room_model.rs` (is_geom_in_panel): 点包含判定（行654-675）
  - 使用parry3d::shape::TriMesh的project_point()方法
  - 判定条件：projection.is_inside || distance_sq <= tolerance_sq
  - 阈值投票：50%的关键点在面板内判定为包含
  - tolerance参数默认为0.1

#### 增强的几何缓存机制

- `src/fast_model/room_model.rs` (load_geometry_with_enhanced_cache): 缓存加载函数（行529-576）
  - 缓存键："{geo_hash}_L0"（使用L0 LOD标识）
  - 缓存结构：DashMap<String, Arc<PlantMesh>>
  - 命中时：从缓存的PlantMesh构建TriMesh
  - 未命中时：从磁盘加载，使用PlantMesh::des_mesh_file()
  - TriMesh标志：ORIENTED | MERGE_DUPLICATE_VERTICES

- `src/fast_model/room_model.rs` (get_enhanced_geometry_cache): 全局缓存获取（行120-124）
  - 使用tokio::sync::OnceCell确保单例
  - DashMap支持并发访问
  - 命中率统计：通过CacheMetrics记录

#### 版本化存储机制

- `src/fast_model/aabb_cache.rs` (put_ref_bbox_versioned): 版本化AABB存储（行493-528）
  - 表：versioned_ref_bbox (refno_key TEXT, session INTEGER, data BLOB)
  - RefnoEnum转换：RefnoSesno::new(refno, session)
  - 时间戳记录：created_at和updated_at（UNIX秒级）
  - 主键：(refno_key, session)组合唯一

- `src/fast_model/aabb_cache.rs` (get_ref_bbox_history): 历史版本查询（行552-588）
  - SQL：`SELECT session, data FROM versioned_ref_bbox WHERE refno_key LIKE ?1 ORDER BY session`
  - 前缀匹配查询refno的所有session版本
  - 返回：Vec<(u32, RStarBoundingBox)>，按session排序

- `src/fast_model/aabb_cache.rs` (get_ref_bbox_at_session): 特定session查询（行530-550）
  - 按refno_key和session组合查询
  - session=0表示当前版本
  - session>0表示历史版本

#### 时间数据存储和映射

- `src/fast_model/aabb_cache.rs` (RefnoTimeData): 时间数据结构（行32-41）
  - 字段：refno_value, session, dbnum, created_at, updated_at, sesno_timestamp, author, description
  - 存储表：refno_time_data (refno_key, session, data)

- `src/fast_model/aabb_cache.rs` (SesnoTimeMapping): Session时间映射（行44-50）
  - 字段：dbnum, sesno, timestamp, description
  - 存储表：sesno_time_mapping (dbnum, sesno, timestamp)
  - 用于追踪每个sesno对应的实际时间

#### 并发控制和性能优化

- `src/fast_model/room_model.rs` (compute_room_relations): 并发处理入口（行174-225）
  - 并发度：buffer_unordered(options.concurrency.max(1))，默认4
  - 环境变量：ROOM_RELATION_CONCURRENCY用于调整
  - 统计信息：RoomBuildStats包含处理时间、缓存命中率、内存使用量

- `src/fast_model/room_model.rs` (CacheMetrics): 缓存统计（行79-111）
  - 原子操作：AtomicU64用于并发计数
  - 命中率计算：hits / (hits + misses)
  - 重置函数：在每次构建开始时清零

#### 数据库连接和配置

- `src/fast_model/aabb_cache.rs` (configure_connection): SQLite连接优化（行173-178）
  - WAL模式：pragma journal_mode = WAL
  - 同步级别：pragma synchronous = NORMAL
  - 缓存大小：pragma cache_size = 10000

- `src/fast_model/aabb_cache.rs` (put_ref_bbox): 原子写入（行452-479）
  - 同时更新两个表：ref_bbox（数据）和aabb_index（RTree）
  - INSERT OR REPLACE策略确保幂等性
  - 6维RTree参数转换：[min/max_x/y/z as f64]

#### SurrealDB关系保存

- `src/fast_model/room_model.rs` (save_room_relate): SurrealDB RELATE语句（行691-697）
  - 关系类型：room_relate（双向有向关系）
  - 关系属性：room_num (房间号)、confidence (0.9)、created_at (系统时间)
  - 语句格式：`relate panel_refno->room_relate:relation_id->refno set room_num='...', confidence=0.9, created_at=time::now();`
  - PE_KEY转换：RefnoEnum.to_pe_key()生成PDMS表达式格式

---

### Report (The Answers)

#### result

**1. SQLite RTree 空间索引的数据结构和查询方式**

SQLite使用虚表(virtual table)实现R-Tree空间索引，具体实现如下：

- **表结构**：6维R-Tree虚表定义在aabb_cache.rs第235-238行
  ```
  CREATE VIRTUAL TABLE aabb_index USING rtree(
      id, min_x, max_x, min_y, max_y, min_z, max_z
  )
  ```
  - `id`：RefU64的64位整数（作为主键）
  - `min_x, max_x, min_y, max_y, min_z, max_z`：AABB的6个坐标值

- **查询机制**：sqlite_query_intersect() (aabb_cache.rs:320-342)
  - 接收查询AABB作为参数
  - 使用6参数相交判定SQL：`WHERE max_x >= ?1 AND min_x <= ?2 AND ...`
  - 返回所有与查询范围重叠的RefU64列表

- **性能特性**：
  - R-Tree支持高效的多维范围查询
  - 自动索引结构优化，无需手动维护
  - 支持部分重叠判定（相交查询）

**2. save_room_relate()函数保存流程**

room_model.rs中的save_room_relate()（行678-711）是房间关系的保存实现：

- **输入参数**：
  - panel_refno：面板参考号
  - within_refnos：HashSet<RefnoEnum>，房间内的构件列表
  - room_num：房间号字符串

- **核心逻辑**：
  1. 遍历within_refnos中的每个构件
  2. 为每个构件生成唯一的relation_id：`format!("{}_{}", panel_refno, refno)`
  3. 构造SurrealDB RELATE语句：`relate panel_refno->room_relate:relation_id->refno set room_num='...', confidence=0.9, created_at=time::now();`
  4. 所有SQL语句用\n拼接成批量语句
  5. 通过SUL_DB.query()一次性执行所有语句

- **关系模式**：
  - 源点(from)：面板(panel_refno)
  - 关系类型：room_relate（自定义关系类型）
  - 目标点(to)：构件(refno)
  - 关系属性：room_num、confidence、created_at

**3. 批量保存的实现细节**

- **批处理策略**（room_model.rs:687-703）：
  - 在内存中累积SQL语句（Vec<String>）
  - 使用换行符分隔多个RELATE语句
  - 一次网络往返提交整个批处理
  - 避免单条关系逐个保存的开销

- **错误处理**（room_model.rs:396-401）：
  - save_room_relate()的错误在process_panel_for_room中捕获
  - 错误日志记录面板和错误信息
  - 继续处理下一个面板（非阻塞设计）

**4. 空间索引的建立过程**

SQLite RTree的建立分为两个阶段：

- **表结构初始化**（aabb_cache.rs:180-246）：
  - 在init_schema()中执行CREATE VIRTUAL TABLE语句
  - 同时创建普通表ref_bbox和其他辅助表
  - 建立索引以加速查询：idx_versioned_refno、idx_time_data_refno

- **数据填充**（aabb_cache.rs:463-476）：
  - put_ref_bbox()方法同时更新两个表：
    1. ref_bbox：存储完整的RStarBoundingBox数据（BLOB）
    2. aabb_index：存储RTree索引坐标（6个float64值）
  - INSERT OR REPLACE策略确保数据一致性

- **重建机制**（aabb_cache.rs:280-317）：
  - sqlite_rebuild_from_internal()从ref_bbox读取所有数据
  - 逐条插入RTree虚表
  - 可用于恢复损坏的索引

**5. 混合索引（内存R-Tree + SQLite）的工作机制**

系统采用两层索引策略，虽然代码中主要使用SQLite，但设计中考虑了内存加速：

- **SQLite层**（磁盘持久化）：
  - 存储所有AABB数据和RTree索引
  - query_overlap()直接查询SQLite
  - 支持大规模几何数据的永久存储

- **内存缓存层**：
  - get_enhanced_geometry_cache()在DashMap中缓存PlantMesh对象
  - 缓存键为"{geo_hash}_L0"
  - 避免重复从磁盘加载网格文件

- **查询流程**（room_model.rs:412-526）：
  1. 粗算阶段：使用SQLite RTree快速找到候选构件
  2. 细算阶段：对候选构件进行关键点精确检测
  3. 结果保存：批量保存到SurrealDB

**6. 数据一致性保证机制**

系统采用多种机制保证数据一致性：

- **原子操作**（aabb_cache.rs:452-479）：
  - put_ref_bbox()同时更新ref_bbox和aabb_index两个表
  - 使用SQLite事务保证原子性

- **版本控制**（aabb_cache.rs:493-550）：
  - 版本化表：versioned_ref_bbox
  - 复合主键：(refno_key, session)
  - 支持时间旅行查询

- **时间戳记录**（aabb_cache.rs:510-517）：
  - created_at：数据创建时刻
  - updated_at：最后修改时刻
  - 支持审计和恢复

- **关系完整性**（room_model.rs:683-685）：
  - 空集合检查：无法保存的关系直接返回
  - 错误日志：所有失败都记录日志

---

#### conclusions

**关键事实总结**

1. **SQLite RTree是核心空间查询基础**
   - 6维虚表实现AABB查间距离检测
   - query_overlap()返回重叠构件列表
   - 典型查询限制：最多1000个候选

2. **房间关系保存采用批量SurrealDB RELATE**
   - 批量SQL降低网络往返开销
   - 关系属性包含room_num、confidence、时间戳
   - 单次调用可保存多个房间-构件关系

3. **两阶段精确性保证策略**
   - 粗算：SQLite RTree快速过滤
   - 细算：27点关键点投票机制（50%阈值）
   - L0 LOD网格减少I/O和内存占用

4. **并发性能设计**
   - buffer_unordered(4)控制并发度
   - DashMap支持无锁并发缓存访问
   - CacheMetrics原子计数器追踪命中率

5. **版本化存储支持增量更新**
   - RefnoSesno绑定refno与session版本
   - 历史查询支持时间范围过滤
   - 复合主键(refno_key, session)保证一致性

6. **数据持久化采用多表并行策略**
   - ref_bbox表：完整BLOB数据
   - aabb_index表：RTree坐标索引
   - 其他表：时间数据、关系映射、版本历史

---

#### relations

**代码关系图**

1. **空间查询链**：
   - cal_room_refnos() (room_model.rs:412)
   - → aios_core::spatial::sqlite::query_overlap() [外部调用]
   - → sqlite_query_intersect() (aabb_cache.rs:320) [实现]
   - → SQLite RTree虚表查询

2. **关系保存链**：
   - compute_room_relations() (room_model.rs:174)
   - → process_panel_for_room() (room_model.rs:376)
   - → cal_room_refnos() (room_model.rs:412) [获取构件列表]
   - → save_room_relate() (room_model.rs:678) [保存关系]
   - → SUL_DB.query() [执行SurrealDB]

3. **几何缓存链**：
   - load_geometry_with_enhanced_cache() (room_model.rs:529)
   - → get_enhanced_geometry_cache() (room_model.rs:120) [获取全局缓存]
   - → PlantMesh::des_mesh_file() [磁盘加载]
   - → TriMesh构建 [parry3d]

4. **精确检测链**：
   - is_geom_in_panel() (room_model.rs:654)
   - → extract_geom_key_points() (room_model.rs:640) [提取关键点]
   - → extract_aabb_key_points() (room_model.rs:595) [AABB展开]
   - → panel_tri_mesh.project_point() [parry3d点投影]

5. **版本管理链**：
   - put_ref_bbox_versioned() (aabb_cache.rs:493) [存储]
   - → get_ref_bbox_history() (aabb_cache.rs:552) [历史查询]
   - → get_ref_bbox_at_session() (aabb_cache.rs:530) [特定版本查询]

6. **数据库连接层**：
   - AabbCache实例 → configure_connection() (aabb_cache.rs:173)
   - → WAL模式、同步策略、缓存配置
   - → SQLite Connection管理

---

### Testing Points (需要验证的数据操作点)

1. **空间索引查询验证点**
   - `src/test/test_sqlite_spatial.rs` - SQLite RTree基本功能测试
   - `src/test/test_room_integration.rs` - 完整房间计算集成测试
   - 验证：query_overlap()返回的候选数量、精确性

2. **关系保存验证点**
   - 验证save_room_relate()是否正确生成RELATE语句
   - 验证SurrealDB中room_relate关系数量和属性
   - 验证batch_sql多语句执行的原子性

3. **几何检测验证点**
   - extract_aabb_key_points()生成的27个点是否正确
   - is_geom_in_panel()的50%阈值投票逻辑
   - L0 LOD网格加载的性能和准确性

4. **版本化存储验证点**
   - put_ref_bbox_versioned()写入的refno_key格式
   - get_ref_bbox_history()返回的排序顺序
   - session=0与session>0的版本区分

5. **并发性能验证点**
   - buffer_unordered(4)的实际并发度测量
   - CACHE_METRICS.hit_rate()的命中率统计
   - estimate_memory_usage()的内存占用评估

6. **数据一致性验证点**
   - ref_bbox和aabb_index表的数据同步
   - versioned_ref_bbox的主键唯一性
   - 事务边界的原子性保证

---

### Implementation Evidence

**关键源文件位置参考**：

| 功能 | 文件 | 行号 |
|------|------|------|
| SQLite RTree定义 | aabb_cache.rs | 235-238 |
| RTree查询 | aabb_cache.rs | 320-342 |
| 关系保存主函数 | room_model.rs | 678-711 |
| 房间构件计算 | room_model.rs | 412-526 |
| 关键点提取 | room_model.rs | 595-650 |
| 精确点检测 | room_model.rs | 654-675 |
| 版本化存储 | aabb_cache.rs | 493-550 |
| 并发处理 | room_model.rs | 174-225 |
| 几何缓存 | room_model.rs | 120-124, 529-576 |
| 数据库配置 | aabb_cache.rs | 173-178 |
