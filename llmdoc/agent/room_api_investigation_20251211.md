<!-- 房间计算 API 接口和调用流程深度调查报告 -->

### Code Sections (The Evidence)

#### 1. Web API 层（REST API 端点）

- `src/web_api/spatial_query_api.rs` (create_spatial_query_routes): REST API 路由工厂函数，创建三个主要端点：
  - `/api/spatial/query/{refno}` - 查询空间节点及其子节点
  - `/api/spatial/children/{refno}` - 查询子节点
  - `/api/spatial/node-info/{refno}` - 获取节点详细信息

- `src/web_api/spatial_query_api.rs` (query_spatial_node): 处理 GET 请求，解析参考号，查询节点信息和子节点。返回 SpatialQueryResponse。

- `src/web_api/spatial_query_api.rs` (query_children_by_type): 根据父节点类型（FRMW/SBFR=SPACE, PANE=ROOM, 其他=COMPONENT）查询子节点，使用特定的 SQL 表关系（room_panel_relate, room_relate, pe_owner）。

#### 2. Web Server 层（房间 API）

- `src/web_server/room_api.rs` (create_room_api_routes): 房间计算 API 路由定义，包括 13 个端点：
  - POST `/api/room/tasks` - 创建房间计算任务
  - GET `/api/room/tasks/{id}` - 获取任务状态
  - GET `/api/room/query` - 查询房间号（单点）
  - POST `/api/room/batch-query` - 批量查询房间号
  - POST `/api/room/process-codes` - 处理房间代码
  - POST `/api/room/regenerate-models` - 重新生成房间模型
  - POST `/api/room/rebuild-relations` - 重建房间关系（不生成模型）
  - GET `/api/room/status` - 获取系统状态
  - POST `/api/room/snapshot` - 创建数据快照

- `src/web_server/room_api.rs` (RoomApiState): API 状态容器，包含任务管理器和进度推送中心

- `src/web_server/room_api.rs` (create_room_task): 创建房间计算任务处理函数：
  1. 生成任务 ID（UUID）
  2. 将任务插入活跃任务列表
  3. 注册到 ProgressHub（用于 WebSocket/gRPC 进度推送）
  4. 异步执行任务（tokio::spawn）
  5. 发布初始进度消息（0%，Pending 状态）

- `src/web_server/room_api.rs` (execute_room_task): 异步任务执行主入口，根据任务类型分派到不同的执行函数：
  - RebuildRelations → execute_rebuild_relations
  - UpdateRoomCodes → execute_update_room_codes
  - DataMigration → execute_data_migration
  - DataValidation → execute_data_validation
  - CreateSnapshot → execute_create_snapshot

- `src/web_server/room_api.rs` (query_room_by_point): 单点房间查询，使用 aios_core 的 query_v2 模块：
  1. 解析查询点坐标
  2. 调用 query_room_number_by_point_v2 获取房间号
  3. 调用 query_room_panel_by_point_v2 获取面板引用号
  4. 计算置信度（0.95=高，0.80=中，0.0=无）
  5. 记录查询耗时和结果

- `src/web_server/room_api.rs` (batch_query_rooms): 批量房间查询（支持并发）：
  1. 使用 batch_query_room_numbers 批量查询（并发度为 8）
  2. 对每个结果获取面板引用号
  3. 返回所有查询结果的集合

- `src/web_server/room_api.rs` (execute_rebuild_relations): 重建房间关系任务执行函数：
  1. 获取全局房间系统管理器（aios_core::RoomSystemManager）
  2. 如果需要强制重建，执行系统清理（cleanup_system）
  3. 按数据库号或全局处理房间关系
  4. 返回处理统计（成功数、错误数、警告）

- `src/web_server/room_api.rs` (execute_update_room_codes): 更新房间代码任务执行函数：
  1. 处理房间关键词（如"-RM"）
  2. 按数据库编号更新房间代码
  3. 使用 RoomSystemManager 进行数据库操作

- `src/web_server/room_api.rs` (get_room_system_status): 获取系统状态，返回健康状态、指标、活跃任务、缓存状态

#### 3. gRPC 服务层（空间查询）

- `src/grpc_service/spatial_query_service.rs` (SpatialQueryServiceImpl): gRPC 空间查询服务实现，支持多种查询和检测：
  - query_intersecting_elements: 查询与指定参考号相交的构件
  - batch_query_intersecting: 批量空间查询
  - rebuild_spatial_index: 重建空间索引
  - get_index_stats: 获取索引统计
  - detect_sctn_contacts: 检测 SCTN（电缆桥架段）接触
  - batch_detect_sctn_contacts: 批量检测接触
  - detect_tray_supports: 检测桥架支撑关系

- `src/grpc_service/spatial_query_service.rs` (SpatialQueryServiceImpl::new): 从 SQLite 空间索引加载初始 R-star 树索引

- `src/grpc_service/spatial_query_service.rs` (query_intersecting_elements): gRPC 空间查询处理：
  1. 解析包围盒和容差参数
  2. 优先使用 SQLite RTree 查询（feature="sqlite-index"）
  3. 回退到内存 R-star 树
  4. 计算交集体积和距离
  5. 按距离排序并限制结果数量

- `src/grpc_service/spatial_query_service.rs` (get_sctn_geometry): 获取 SCTN 几何信息，支持从数据库提取

#### 4. 核心 API 数据模型

- `src/web_api/spatial_query_api.rs` (SpatialNode): 空间节点数据结构（refno, name, noun, node_type, children_count）

- `src/web_api/spatial_query_api.rs` (SpatialQueryResponse): 空间查询响应（success, node, children, error_message）

- `src/web_api/spatial_query_api.rs` (determine_node_type): 节点类型判断函数：
  - FRMW / SBFR → "SPACE"（空间）
  - PANE → "ROOM"（房间）
  - 其他 → "COMPONENT"（构件）

- `src/web_server/room_api.rs` (RoomComputeTask): 房间计算任务数据结构（id, task_type, status, progress, config, result）

- `src/web_server/room_api.rs` (RoomQueryResponse): 房间查询响应（success, room_number, panel_refno, confidence, query_time_ms）

- `src/web_server/room_api.rs` (RoomComputeConfig): 房间计算配置（project_code, room_keywords, database_numbers, force_rebuild, batch_size, validation_options, model_generation）

- `src/web_server/room_api.rs` (ModelGenerationOptions): 模型生成选项（generate_model, generate_mesh, generate_spatial_tree, apply_boolean_operation, output_formats, quality_level）

- `src/web_server/room_api.rs` (RoomSystemStatusResponse): 系统状态响应（system_health, metrics, active_tasks, cache_status）

#### 5. Web Server 路由集成

- `src/web_server/mod.rs` (start_web_server_with_config): 启动 Web 服务器并注册所有路由：
  1. 创建 SpatialQueryApiState 和 room_api::RoomApiState
  2. 创建对应的路由（create_spatial_query_routes, create_room_api_routes）
  3. 合并所有路由到主 Axum Router
  4. 绑定到 0.0.0.0:{port} 并启动服务

- `src/web_server/mod.rs` (line 210-216): 房间 API 路由创建和状态注入

#### 6. 数据库查询层

- `src/api/room_code.rs` (query_room_code): 单个参考号查询房间号，使用 ROOM_CODE 表

- `src/api/room_code.rs` (query_room_code_with_refnos): 批量参考号查询房间号

- `src/api/room_code.rs` (query_room_nodes): 按数据库编号查询所有房间节点，支持命名约定过滤（如"-RM"后缀）

#### 7. 快速模型和房间模型实现

- `src/fast_model/room_model.rs`: 房间关系构建的核心实现，包含：
  - RoomBuildStats: 房间构建统计
  - RoomComputeOptions: 房间计算选项（容差、并发度）
  - CacheMetrics: 缓存命中率统计

#### 8. 进度推送机制

- `src/web_server/room_api.rs` (create_room_task): 使用 ProgressHub 注册任务并发布进度消息
- `src/web_server/room_api.rs` (execute_room_task): 在任务执行过程中发布进度更新
- 使用 ProgressMessageBuilder 构建进度消息（状态、百分比、步骤、消息）

#### 9. 错误处理和功能特性

- REST API 使用 Result<Json<T>, StatusCode> 返回类型
- gRPC 使用 Result<Response<T>, Status> 返回类型
- 支持条件编译特性：
  - `feature = "sqlite"` - 启用 SQLite 空间索引查询
  - `feature = "sqlite-index"` - 启用 AABB 缓存和索引
  - 无特性时提供占位符实现以保持 API 可用

### Report (The Answers)

#### result

**1. 对外提供的房间查询 API 端点**

主要通过两个 Web API 模块提供对外接口：

**空间查询 API** (REST):
- `GET /api/spatial/query/{refno}` - 查询空间节点及其子节点，支持递归查询
- `GET /api/spatial/children/{refno}` - 单独查询子节点
- `GET /api/spatial/node-info/{refno}` - 获取节点详细信息（包括 owner 关系）

**房间计算 API** (REST):
- `GET /api/room/query?point=[x,y,z]&tolerance=0.1&max_results=10` - 单点房间查询
- `POST /api/room/batch-query` - 批量房间查询（支持多个点）
- `POST /api/room/process-codes` - 处理和标准化房间代码
- `POST /api/room/tasks` - 创建房间计算任务（重建、更新、迁移、验证）
- `GET /api/room/tasks/{task_id}` - 查询任务执行状态
- `GET /api/room/status` - 获取房间系统整体状态（健康状态、缓存、指标）
- `POST /api/room/snapshot` - 创建数据快照
- `POST /api/room/regenerate-models` - 重新生成房间模型
- `POST /api/room/rebuild-relations` - 仅重建房间关系（不生成模型）

**gRPC 服务**（空间查询）:
- `SpatialQueryService::query_intersecting_elements` - 查询相交构件
- `SpatialQueryService::batch_query_intersecting` - 批量查询
- `SpatialQueryService::detect_sctn_contacts` - 检测电缆桥架接触
- `SpatialQueryService::detect_tray_supports` - 检测桥架支撑关系

**2. Web API 层实现（spatial_query_api.rs）**

Web API 层由 Axum 框架实现，包含以下关键函数：

- `create_spatial_query_routes()`: 工厂函数，创建 Router 并注册三个 GET 端点
- `query_spatial_node()`: 处理 `/api/spatial/query/{refno}` 请求
  - 从路径参数解析 refno
  - 执行 SELECT 查询获取节点（pe 表）
  - 根据节点的 noun 字段确定节点类型
  - 调用 `query_children_by_type()` 获取子节点
  - 返回 SpatialQueryResponse（包含节点、子节点列表、错误信息）

- `query_children_by_type()`: 核心的子节点查询逻辑
  - FRMW/SBFR（框架）→ 通过 room_panel_relate 表查询房间
  - PANE（房间）→ 通过 room_relate 表查询构件
  - 其他类型 → 通过 owner 字段查询直属子节点
  - 返回最多 100 个子节点

- `get_node_info()`: 获取单个节点详细信息，包括 owner 引用

**3. gRPC 服务接口**

gRPC 服务使用 Tonic 框架实现，主要特点：

- 使用 Protobuf 定义服务接口（proto/spatial_query.proto）
- 支持 R-star 树空间索引（parry3d + rstar crate）
- 支持 SQLite AABB 缓存（feature="sqlite-index"）
- 查询流程：
  1. 优先使用 SQLite RTree 进行空间查询
  2. 若无索引或查询为空，回退到内存 R-star 树
  3. 计算相交体积、距离等属性
  4. 按距离排序并限制结果数量

**4. 完整的请求-响应流程（单点房间查询为例）**

```
客户端 HTTP GET /api/room/query?point=[1.0,2.0,3.0]
  ↓
query_room_by_point() 处理器
  ├─ 解析查询点坐标为 Vec3
  ├─ 调用 aios_core::room::query_v2::query_room_number_by_point_v2(point)
  │  └─ [SQLite 特性] 使用混合索引查询（房间号数据库）
  ├─ 调用 query_room_panel_by_point_v2(point) 获取面板 refno
  ├─ 计算置信度：
  │  ├─ 找到房间号 + 面板 → 0.95
  │  ├─ 仅房间号 → 0.80
  │  └─ 无结果 → None
  ├─ 记录日志（查询耗时、坐标、结果）
  └─ 返回 RoomQueryResponse
    {
      success: bool,
      room_number: Option<String>,
      panel_refno: Option<u64>,
      confidence: Option<f64>,
      query_time_ms: f64
    }
```

**批量房间查询流程**：
- 支持并发度为 8 的批量查询
- 对每个点独立执行上述流程
- 累计总查询耗时
- 返回所有点的查询结果列表

**房间计算任务流程**：
```
客户端 POST /api/room/tasks
  ↓
create_room_task() 处理器
  ├─ 生成任务 ID (UUID)
  ├─ 创建 RoomComputeTask（初始状态：Pending）
  ├─ 插入活跃任务列表
  ├─ 注册到 ProgressHub（进度推送中心）
  ├─ 发布初始进度消息（0%, Pending）
  ├─ 异步执行任务
  └─ 立即返回任务对象
    ↓ [异步执行]
    execute_room_task()
      ├─ 更新状态 → Running
      ├─ 发布运行中消息（0%）
      ├─ 根据 task_type 分派：
      │  ├─ RebuildRelations → execute_rebuild_relations()
      │  │  ├─ 调用 RoomSystemManager::cleanup_system()（可选）
      │  │  ├─ 按数据库处理房间关系重建
      │  │  └─ 返回 RoomComputeResult
      │  ├─ UpdateRoomCodes → execute_update_room_codes()
      │  ├─ DataMigration → execute_data_migration()
      │  ├─ DataValidation → execute_data_validation()
      │  └─ CreateSnapshot → execute_create_snapshot()
      ├─ 更新任务结果和状态
      ├─ 发布完成/失败消息（100%）
      ├─ 移动到历史记录
      └─ 从 ProgressHub 注销
    ↓
客户端轮询 GET /api/room/tasks/{task_id} 获取状态
  ↓
get_task_status() 处理器
  ├─ 在活跃任务列表中查找
  ├─ 若不存在，检查历史记录
  └─ 返回 RoomComputeTask（包含最新状态和结果）
```

**WebSocket 进度推送**：
- 客户端连接到 `/ws/progress/{task_id}`
- ProgressHub 向所有订阅的 WebSocket 连接推送进度消息
- 消息格式：ProgressMessage（task_id, status, percentage, step, message）

**5. 错误处理和异常情况**

- **REST API 错误**：
  - 无效参考号格式 → StatusCode::BAD_REQUEST
  - 节点不存在 → SpatialQueryResponse { success: false, error_message: "Node not found" }
  - 数据库查询失败 → 返回错误消息并记录日志

- **房间查询错误**：
  - 无 SQLite 特性 → 使用占位符实现（生成假的房间号）
  - 查询超时 → 记录错误并返回失败状态

- **gRPC 错误**：
  - 索引文件无效 → Err(anyhow::anyhow!(...))
  - 构件不存在 → SpatialQueryResponse { success: false, error_message: ... }
  - 检测器创建失败 → 返回 Status 错误

- **任务执行错误**：
  - 任务 ID 不存在 → StatusCode::NOT_FOUND
  - 系统清理失败 → 记录到 errors 列表，继续处理
  - 数据库操作失败 → 记录到 errors 并标记为 Failed

**6. 前端集成示例**

前端可通过以下方式集成：

**REST API 调用（JavaScript/TypeScript）**：
```javascript
// 单点房间查询
const response = await fetch('/api/room/query?point=[1.0,2.0,3.0]&tolerance=0.1');
const data = await response.json();
console.log(data.room_number, data.confidence);

// 批量查询
const batchResponse = await fetch('/api/room/batch-query', {
  method: 'POST',
  body: JSON.stringify({
    points: [[1.0,2.0,3.0], [4.0,5.0,6.0]],
    tolerance: 0.1
  })
});

// 创建计算任务
const taskResponse = await fetch('/api/room/tasks', {
  method: 'POST',
  body: JSON.stringify({
    task_type: 'RebuildRelations',
    config: {
      project_code: 1516,
      force_rebuild: true,
      database_numbers: [7999],
      validation_options: { check_room_codes: true },
      model_generation: { generate_model: true }
    }
  })
});
const task = await taskResponse.json();
console.log('Task ID:', task.id);

// 轮询任务状态
setInterval(async () => {
  const statusResponse = await fetch(`/api/room/tasks/${task.id}`);
  const taskStatus = await statusResponse.json();
  console.log('Progress:', taskStatus.progress, '%');
}, 1000);
```

**WebSocket 进度监听**：
```javascript
const ws = new WebSocket(`ws://localhost:8080/ws/progress/${task_id}`);
ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  console.log(`${msg.step}: ${msg.percentage}%`);
};
```

**gRPC 调用（TypeScript with grpc-web）**：
```typescript
import { SpatialQueryServiceClient } from './spatial_query_pb_service';
import { SpatialQueryRequest } from './spatial_query_pb';

const client = new SpatialQueryServiceClient('http://localhost:50051');
const request = new SpatialQueryRequest();
request.setRefno(12345);
request.setElementTypesList(['TUBI', 'ELBO']);
request.setTolerance(0.001);

client.queryIntersectingElements(request, {}, (err, response) => {
  if (err) console.error(err);
  else console.log('Found elements:', response.getElementsList().length);
});
```

#### conclusions

**关键事实总结**：

1. **双层 API 架构**：系统同时提供 REST API（Axum）和 gRPC 服务（Tonic），满足不同客户端需求

2. **空间查询支持层次化导航**：
   - 空间 → 房间 → 构件的三级关系
   - 通过不同的数据库表管理关系（room_panel_relate, room_relate, pe_owner）
   - 支持递归查询子节点

3. **房间查询的核心流程**：
   - 依赖 aios_core 的 query_v2 模块（query_room_number_by_point_v2）
   - 优先使用 SQLite 混合索引获取房间号
   - 支持单点和批量查询（并发度 8）
   - 置信度计算基于查询完整性（同时找到房间号和面板 = 95%）

4. **异步任务执行框架**：
   - 房间计算任务通过 tokio::spawn 异步执行
   - 支持 5 种任务类型（RebuildRelations, UpdateRoomCodes, DataMigration, DataValidation, CreateSnapshot）
   - 实时进度推送通过 ProgressHub 和 WebSocket 实现
   - 任务历史保留最近 100 条

5. **空间索引优化**：
   - 优先使用 SQLite AABB 缓存（feature="sqlite-index"）
   - 回退到内存 R-star 树（parry3d + rstar）
   - 支持 SCTN（电缆桥架段）接触检测和支撑关系分析

6. **容错和降级策略**：
   - 无 SQLite 特性时提供占位符实现保持 API 可用
   - 数据库查询失败自动记录并返回错误响应
   - gRPC 和 REST API 均有完善的错误处理

7. **配置和验证选项**：
   - 支持按项目代码、关键词、数据库编号过滤处理
   - 支持强制重建、批处理大小、模型生成选项配置
   - 验证选项包括房间代码检查、空间一致性检查、引用完整性检查

#### relations

**API 调用链关系**：

1. **REST API 请求入口** (`src/web_server/mod.rs`)
   - 路由注册时创建 `SpatialQueryApiState` 和 `RoomApiState`
   - 通过 `create_spatial_query_routes()` 和 `create_room_api_routes()` 生成路由
   - 合并到主 Axum Router 并启动服务

2. **空间查询 API** → **数据库查询层**
   - `query_spatial_node()` 调用 `determine_node_type()` 判断节点类型
   - `query_spatial_node()` 调用 `query_children_by_type()` 获取子节点
   - `query_children_by_type()` 执行 SurrealDB SQL 查询（通过 `SUL_DB.query_take()`）
   - 涉及的数据库表：pe（节点），room_panel_relate（空间-房间关系），room_relate（房间-构件关系）

3. **房间查询 API** → **aios_core 房间查询模块**
   - `query_room_by_point()` 调用 `aios_core::room::query_v2::query_room_number_by_point_v2()`
   - 调用 `aios_core::room::query_v2::query_room_panel_by_point_v2()` 获取面板
   - 调用 `aios_core::room::query_v2::get_room_query_stats()` 获取缓存统计
   - 所有查询依赖 SQLite 混合索引（当 feature="sqlite" 启用）

4. **房间任务管理** → **房间系统管理器**
   - `create_room_task()` 注册任务到 ProgressHub
   - `execute_room_task()` 根据任务类型调用不同的执行函数
   - 执行函数调用 `aios_core::room::room_system_manager::get_global_manager()`
   - 通过 RoomSystemManager 执行数据库操作（cleanup_system, migrate_legacy_data, validate_system_data）

5. **进度推送流程**
   - 任务创建时注册到 ProgressHub（`progress_hub.register(task_id)`）
   - 任务执行中发布进度消息（`progress_hub.publish(message)`）
   - WebSocket 客户端订阅 `/ws/progress/{task_id}`
   - ProgressHub 向所有订阅者推送消息
   - 任务完成后注销（`progress_hub.unregister(task_id)`）

6. **gRPC 空间查询** → **空间索引和几何提取**
   - `query_intersecting_elements()` 优先使用 SQLite RTree 查询
   - 回退到内存 R-star 树（从 SQLite 数据库加载初始化）
   - 调用 `get_element_bbox()` 获取元素包围盒
   - 调用 `calculate_intersection_volume()` 计算交集体积
   - SCTN 接触检测调用 `get_sctn_geometry()` 和 `get_branch_sections()`

7. **数据库层关系**
   - SpatialQueryAPI 直接查询 SurrealDB（pe 表）
   - 房间计算 API 依赖 aios_core（调用房间系统管理器）
   - 房间系统管理器可能查询 SurrealDB 或 SQLite（取决于特性启用）
   - gRPC 服务可选地使用 TiDB 管理器（AiosDBManager）获取几何数据

8. **特性交叉依赖**
   - 房间查询需要 feature="sqlite" 以启用真实查询逻辑，否则使用占位符
   - gRPC 空间索引需要 feature="sqlite-index" 以启用 AABB 缓存
   - Web Server 特性启用时，所有 API 路由可用
   - 任务执行可能涉及模型生成（需要 feature="gen_model"）

---

**调查完成时间**: 2025-12-11
**调查工具**: scout agent
**数据来源**: 源代码分析（REST API、gRPC 服务、房间计算任务管理）
