# 房间计算 V2 流程说明

## 范围与入口
- 主入口：`build_room_relations_v2`（`src/fast_model/room_model_v2.rs`）
- 辅助入口：`rebuild_room_relations_for_rooms`（指定房间重算）、`update_room_relations_incremental`（指定元素增量刷新）
- 依赖：`sqlite-index` 特性、SurrealDB (`SUL_DB`)、本地 SQLite 空间索引、L0 级别 Mesh 文件

## 前置条件
- 编译/运行：`--features sqlite-index`，非 `wasm32` 目标
- 数据：`DbOption` 中的 mesh 路径下存在 L0 Mesh（默认 `assets/meshes/lod_L0`），SQLite 空间索引已由模型生成流程写入，SurrealDB 表包含 FRMW/SBFR 与 PANE 数据
- 配置：房间关键词由 `DbOption::get_room_key_word` 提供；并发度默认 4，可通过环境变量 `ROOM_RELATION_CONCURRENCY` 调整（>0 生效）
- 结果表：`room_panel_relate`（房间→面板关系），`room_relate`（面板→构件关系，含 `room_num`、`confidence`）

## 主流程（build_room_relations_v2）
1) **查询房间与面板**  
   `build_room_panels_relate_v2` 按关键词拼接 Surreal SQL（默认 FRMW；`project_hd`/`project_hh` 特性切换集合），房间号可选正则过滤（`match_room_name_hd` 等）。结果写入 `room_panel_relate`，返回 `(room_refno, room_num, panel_refnos)`。
2) **排除集准备**  
   收集所有房间面板形成 `exclude_panel_refnos`，用于后续粗算时跳过其他面板。
3) **房间并发处理**  
   默认并发 `RoomComputeOptions::concurrency`（环境可调）。每个房间的面板依次调用 `process_panel_for_room`，总体并发通过 `buffer_unordered` 控制。
4) **面板几何准备** (`process_panel_for_room`)  
   - `query_insts` 获取实例与世界矩阵  
   - `load_geometry_with_enhanced_cache` 加载 L0 Mesh，`ENHANCED_GEOMETRY_CACHE` 记录命中率并做简单淘汰
5) **粗算过滤** (`cal_room_refnos_v2`)  
   使用面板世界 AABB 调用 `aios_core::spatial::sqlite::query_overlap`（限制 1000 条）获取候选构件，排除所有房间面板。
6) **细算判定**  
   对候选构件 AABB 抽取 27 个关键点（8 顶点 + 中心 + 面中心 + 边中心），通过 `TriMesh::project_point` 判断点在面板内或距离 ≤ `inside_tol`（默认 0.1），多数票 (>50%) 判定归属。
7) **结果落库**  
   命中构件写入 `room_relate`：`panel -> room_relate:{panel_refno}_{refno} -> component`，附带 `room_num`、`confidence=0.9`。统计 `RoomBuildStats`（房间数、面板数、构件数、耗时、缓存命中率、缓存估算占用）。

## 衍生流程
- **重建指定房间**：`rebuild_room_relations_for_rooms` 先删除对应面板的旧 `room_relate` 关系，再复用主流程计算。
- **增量更新指定元素**：`update_room_relations_incremental` 查找包含目标构件的面板（`query_panels_containing_refnos`），删除旧关系后按面板重算，仍使用退避排除集与并发控制。

## 观测与验证
- 日志关键点：粗算/细算耗时、候选数与命中数、缓存命中率、最终统计；Mesh 加载失败或空间索引返回为空会有 `warn!`。
- 快速验证：`cargo test --features sqlite-index test_room_v2_with_lod_verification -- --ignored --nocapture` 或 `scripts/test/test_room_v2_verification.sh`（若存在）。
- 典型输出表：`room_panel_relate` 建立房间-面板关系；`room_relate` 落库面板-构件关系，供 Web/API 查询。

## 待关注/改进建议
- 运行前缺少 Mesh/L0 路径与 SQLite 文件的显式检查，可在入口提前 fail-fast。
- 空间索引返回上限固定为 1000，超大模型可能需要按场景调参或分片。
- 缓存清理策略较粗（超过 2000 条移除一半），可以引入 LRU/基于大小的淘汰以减少抖动。
- 仍采用“尽量继续”的容错策略（单面板失败不会中断整体），需要结合业务决定是否在批量失败时中止并回滚。
