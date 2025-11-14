# 房间计算系统 Web API 设计文档

## 📋 概述

房间计算系统提供了完整的 REST API 接口，支持房间关系计算、空间查询、数据管理和系统监控等功能。

## 🏗️ 系统架构

### 核心组件
- **房间系统管理器** (`RoomSystemManager`): 统一管理所有房间计算功能
- **空间索引系统** (`HybridSpatialIndex`): SQLite R-tree + 内存 R*-tree 混合索引
- **房间代码处理器** (`RoomCodeProcessor`): 标准化房间代码格式
- **数据迁移工具** (`MigrationTool`): 数据模型迁移和验证
- **版本控制系统** (`VersionControl`): 数据变更追踪和回滚

### 技术栈
- **后端框架**: Axum (Rust)
- **数据库**: SurrealDB + SQLite (空间索引)
- **空间计算**: Parry3D + R-tree
- **前端**: HTML + TailwindCSS + Vanilla JavaScript

## 🔌 API 接口设计

### 1. 房间查询接口

#### 单点房间查询
```http
GET /api/room/query?point=x,y,z&tolerance=0.001
```

**参数:**
- `point`: 3D坐标点 `[x,y,z]`
- `tolerance`: 查询容差 (可选)

**响应:**
```json
{
  "success": true,
  "room_number": "SSC-A001",
  "panel_refno": 12345,
  "confidence": 0.95,
  "query_time_ms": 0.5
}
```

#### 批量房间查询
```http
POST /api/room/batch-query
Content-Type: application/json

{
  "points": [[x1,y1,z1], [x2,y2,z2]],
  "tolerance": 0.001
}
```

**响应:**
```json
{
  "success": true,
  "results": [
    {
      "success": true,
      "room_number": "SSC-A001",
      "query_time_ms": 0.3
    }
  ],
  "total_query_time_ms": 1.2
}
```

### 2. 房间代码处理接口

#### 房间代码标准化
```http
POST /api/room/process-codes
Content-Type: application/json

{
  "codes": ["SSC-A001", "ssc-a1001", "HD-B123"],
  "project_type": "SSC"
}
```

**响应:**
```json
{
  "success": true,
  "results": [
    {
      "input": "ssc-a1001",
      "success": true,
      "standardized_code": "SSC-A001",
      "project_prefix": "SSC",
      "area_code": "A",
      "room_number": "001",
      "errors": [],
      "warnings": ["格式已标准化"]
    }
  ],
  "processing_time_ms": 2.1
}
```

### 3. 任务管理接口

#### 创建房间计算任务
```http
POST /api/room/tasks
Content-Type: application/json

{
  "task_type": "RebuildRelations",
  "config": {
    "room_keywords": ["-RM", "-ROOM"],
    "database_numbers": [1516, 7999],
    "force_rebuild": false,
    "validation_options": {
      "check_room_codes": true,
      "check_spatial_consistency": true,
      "check_reference_integrity": true
    }
  }
}
```

**任务类型:**
- `RebuildRelations`: 重建房间关系
- `UpdateRoomCodes`: 更新房间代码
- `DataMigration`: 数据迁移
- `DataValidation`: 数据验证
- `CreateSnapshot`: 创建快照

**响应:**
```json
{
  "id": "task_uuid",
  "task_type": "RebuildRelations",
  "status": "Pending",
  "progress": 0.0,
  "message": "任务已创建",
  "created_at": "2025-11-13T12:00:00Z",
  "config": { ... }
}
```

#### 查询任务状态
```http
GET /api/room/tasks/{task_id}
```

**响应:**
```json
{
  "id": "task_uuid",
  "status": "Running",
  "progress": 45.2,
  "message": "正在处理数据库 1516...",
  "result": {
    "success": true,
    "processed_count": 1250,
    "error_count": 3,
    "statistics": {
      "total_rooms": 450,
      "total_panels": 1200,
      "total_relations": 3500,
      "avg_confidence": 0.92
    }
  }
}
```

### 4. 系统监控接口

#### 获取系统状态
```http
GET /api/room/status
```

**响应:**
```json
{
  "system_health": "正常",
  "metrics": {
    "system": {
      "memory_usage_mb": 256.5,
      "cpu_usage_percent": 15.2
    },
    "query": {
      "total_queries": 15420,
      "avg_query_time_ms": 0.8,
      "queries_per_second": 125.3
    },
    "cache": {
      "geometry_cache_hit_rate": 0.85,
      "query_cache_hit_rate": 0.92
    }
  },
  "active_tasks": 2,
  "cache_status": {
    "geometry_cache_size": 1024,
    "query_cache_size": 512,
    "hit_rate": 0.88
  }
}
```

### 5. 数据管理接口

#### 创建数据快照
```http
POST /api/room/snapshot
Content-Type: application/json

"快照描述信息"
```

**响应:**
```json
{
  "success": true,
  "operation_id": "op_uuid",
  "message": "快照创建成功: snapshot_123",
  "timestamp": "2025-11-13T12:00:00Z"
}
```

## 🖥️ Web 界面设计

### 房间计算管理页面 (`/room-management`)

#### 功能模块
1. **系统状态仪表板**
   - 系统健康状态
   - 活跃任务数量
   - 查询性能指标
   - 缓存命中率

2. **房间空间查询**
   - 3D坐标输入 (X, Y, Z)
   - 实时查询结果显示
   - 查询性能统计

3. **房间代码标准化**
   - 批量代码输入
   - 项目类型选择
   - 处理结果展示
   - 错误和警告提示

4. **任务管理中心**
   - 创建新任务
   - 任务状态监控
   - 进度实时更新
   - 历史任务查看

#### 界面特性
- **响应式设计**: 支持桌面和移动设备
- **实时更新**: 自动刷新状态和进度
- **交互友好**: 清晰的操作反馈和错误提示
- **现代UI**: 使用 TailwindCSS 的现代设计风格

## 🔧 配置参数

### 房间计算配置
```json
{
  "project_code": "SSC",
  "room_keywords": ["-RM", "-ROOM"],
  "database_numbers": [1516, 7999],
  "force_rebuild": false,
  "batch_size": 1000,
  "validation_options": {
    "check_room_codes": true,
    "check_spatial_consistency": true,
    "check_reference_integrity": true
  }
}
```

### 验证选项
- `check_room_codes`: 验证房间代码格式
- `check_spatial_consistency`: 验证空间一致性
- `check_reference_integrity`: 验证引用完整性

## 📊 性能指标

### 查询性能
- **单点查询**: < 1ms (平均 0.5ms)
- **批量查询**: 支持并发处理
- **吞吐量**: > 1000 查询/秒
- **缓存命中率**: > 85%

### 系统资源
- **内存使用**: < 500MB (正常负载)
- **CPU使用**: < 20% (查询高峰)
- **存储**: SQLite 索引 + SurrealDB 数据

## 🛡️ 错误处理

### HTTP 状态码
- `200 OK`: 请求成功
- `400 Bad Request`: 请求参数错误
- `404 Not Found`: 资源不存在
- `500 Internal Server Error`: 服务器内部错误

### 错误响应格式
```json
{
  "success": false,
  "error": "错误描述",
  "code": "ERROR_CODE",
  "details": { ... }
}
```

## 🚀 部署和使用

### 启动服务
```bash
# 启动 Web 服务器 (默认端口 3000)
cargo run --bin web_server --features web_server

# 访问房间管理页面
http://localhost:3000/room-management
```

### API 测试示例
```bash
# 查询房间
curl "http://localhost:3000/api/room/query?point=100,200,50"

# 处理房间代码
curl -X POST http://localhost:3000/api/room/process-codes \
  -H "Content-Type: application/json" \
  -d '{"codes": ["SSC-A001", "HD-B102"]}'

# 创建计算任务
curl -X POST http://localhost:3000/api/room/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "task_type": "RebuildRelations",
    "config": {
      "room_keywords": ["-RM"],
      "database_numbers": [1516],
      "force_rebuild": false
    }
  }'
```

## 📈 扩展计划

### 短期目标
- [ ] 完善任务执行逻辑
- [ ] 添加更多验证规则
- [ ] 优化查询性能
- [ ] 增加监控指标

### 长期目标
- [ ] 机器学习辅助房间识别
- [ ] 分布式计算支持
- [ ] 高级可视化界面
- [ ] 自动化运维功能

## 🔗 相关文档

- [房间计算系统架构](./docs/architecture/ROOM_SYSTEM_ARCHITECTURE.md)
- [SQLite 空间索引使用指南](./docs/database/SQLITE_SPATIAL_INDEX.md)
- [API 接口测试](./docs/api/API_TESTING.md)
- [部署运维手册](./docs/deployment/DEPLOYMENT_GUIDE.md)
