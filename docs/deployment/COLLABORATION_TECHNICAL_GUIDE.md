# 异地协同技术完整指南

## 📚 目录

1. [系统架构](#系统架构)
2. [核心概念](#核心概念)
3. [数据模型](#数据模型)
4. [API 完整文档](#api-完整文档)
5. [同步机制](#同步机制)
6. [UI 改造方案](#ui-改造方案)
7. [数据映射关系](#数据映射关系)
8. [实施步骤](#实施步骤)
9. [最佳实践](#最佳实践)

---

## 系统架构

### 整体架构图

```
┌─────────────────────────────────────────────────────────────┐
│                        前端层                                 │
│  ┌────────────────┐              ┌────────────────────┐     │
│  │  Alpine.js UI  │              │  React/Next.js UI  │     │
│  │  (现有实现)     │              │  (新设计方案)       │     │
│  └────────┬───────┘              └─────────┬──────────┘     │
│           │                                │                 │
│           │        HTTP REST API           │                 │
└───────────┼────────────────────────────────┼─────────────────┘
            │                                │
┌───────────▼────────────────────────────────▼─────────────────┐
│                     后端层 (Axum)                             │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  remote_sync_handlers.rs                            │    │
│  │  - 环境管理 (RemoteSyncEnv)                         │    │
│  │  - 站点管理 (RemoteSyncSite)                        │    │
│  │  - 运行时控制 (activate/stop)                       │    │
│  └──────────────────┬──────────────────────────────────┘    │
│                     │                                        │
│  ┌──────────────────▼────────────────────────────────┐      │
│  │  sync_control_handlers.rs                         │      │
│  │  - 启动/停止/暂停/恢复                              │      │
│  │  - 任务队列管理                                     │      │
│  │  - 事件流 (SSE)                                    │      │
│  │  - 性能监控                                        │      │
│  └──────────────────┬────────────────────────────────┘      │
│                     │                                        │
│  ┌──────────────────▼────────────────────────────────┐      │
│  │  sync_control_center.rs                           │      │
│  │  - 全局同步控制中心 (单例)                          │      │
│  │  - 任务队列 (VecDeque)                             │      │
│  │  - 事件广播 (broadcast channel)                    │      │
│  │  - 状态管理                                        │      │
│  └──────────────────┬────────────────────────────────┘      │
│                     │                                        │
│  ┌──────────────────▼────────────────────────────────┐      │
│  │  remote_runtime.rs                                │      │
│  │  - AiosDBManager 实例                              │      │
│  │  - 文件监控 (inotify/FSEvents)                     │      │
│  │  - MQTT 客户端                                     │      │
│  └─────────────────────────────────────────────────┬─┘      │
└────────────────────────────────────────────────────┼────────┘
                                                     │
┌────────────────────────────────────────────────────┼────────┐
│                     存储层                          │         │
│  ┌──────────────────────┐    ┌──────────────────┐ │         │
│  │  SQLite              │    │  MQTT Broker     │ │         │
│  │  - remote_sync_envs  │    │  (外部服务)       │ │         │
│  │  - remote_sync_sites │    │  - QoS 2         │ │         │
│  │  (复用部署站点库)     │    │  - Retain        │ │         │
│  └──────────────────────┘    └──────────────────┘ │         │
│                                                     │         │
│  ┌──────────────────────────────────────────────┐  │         │
│  │  文件系统                                      │  │         │
│  │  - E3D 数据库文件 (.db)                       │  │         │
│  │  - CBA 归档文件 (.cba)                        │  │         │
│  │  - DbOption.toml 配置                        │  │         │
│  └──────────────────────────────────────────────┘  │         │
└─────────────────────────────────────────────────────────────┘
```

### 技术栈

**后端（Rust）**
- Web 框架：Axum
- 数据库：SQLite (rusqlite)
- MQTT 客户端：rumqttc
- 文件监控：notify
- 异步运行时：Tokio
- 序列化：serde + serde_json

**前端（现有）**
- 框架：Alpine.js 3.x
- 样式：Tailwind CSS
- 图标：Font Awesome
- 渲染：服务端模板字符串

**前端（新设计）**
- 框架：Next.js 14 + React 18
- 语言：TypeScript 5.x
- 样式：Tailwind CSS + shadcn/ui
- 状态：React Hooks
- 构建：Turbopack

---

## 核心概念

### 1. RemoteSyncEnv（远程增量环境）

**定义：** 代表一个地理位置或逻辑分组的同步环境。

**职责：**
- 配置 MQTT 连接信息
- 指定文件服务器地址
- 定义地区标识和负责的数据库编号
- 管理断线重连策略

**示例场景：**
```
环境名称：北京数据中心
location：bj
MQTT：mqtt.bj.example.com:1883
文件服务：http://fileserver.bj.example.com:8080/assets/archives
负责 DB：7999, 8001, 8002, 8003
```

### 2. RemoteSyncSite（远程站点）

**定义：** 环境下的具体外部站点，用于从其他地区获取数据。

**职责：**
- 指定外部站点的 HTTP 访问地址
- 定义要同步的数据库编号列表
- 关联到父环境

**示例场景：**
```
站点名称：上海站点-A
所属环境：北京数据中心
HTTP：http://shanghai-site-a.example.com:8080
同步 DB：8010, 8011, 8012
```

### 3. SyncControlCenter（同步控制中心）

**定义：** 全局单例，管理整个同步服务的生命周期和状态。

**核心能力：**
- ✅ 任务队列管理（优先级、并发控制）
- ✅ 实时事件广播（SSE）
- ✅ 性能监控（CPU、内存、速率）
- ✅ 错误恢复（自动重试、断线重连）
- ✅ 状态持久化

### 4. 同步任务生命周期

```
┌────────┐
│ Pending │ ──────┐
└────────┘       │
     ↓           │
┌────────┐       │  自动重试
│ Running │       │  (max 3次)
└────────┘       │
     ↓           ↓
┌──────────┐  ┌────────┐
│ Completed│  │ Failed │
└──────────┘  └────────┘
     ↑           ↑
     │           │
     └───────────┘
      Cancelled
```

---

## 数据模型

### 数据库表结构

#### remote_sync_envs（环境表）

```sql
CREATE TABLE remote_sync_envs (
    id TEXT PRIMARY KEY,                  -- UUID
    name TEXT NOT NULL,                   -- 环境名称
    mqtt_host TEXT,                       -- MQTT 服务器地址
    mqtt_port INTEGER,                    -- MQTT 端口（默认 1883）
    file_server_host TEXT,                -- 文件服务器地址
    location TEXT,                        -- 地区标识 (bj/sjz/zz)
    location_dbs TEXT,                    -- 本地负责的数据库编号（逗号分隔）
    reconnect_initial_ms INTEGER,         -- 重连初始间隔（毫秒）
    reconnect_max_ms INTEGER,             -- 重连最大间隔（毫秒）
    created_at TEXT NOT NULL,             -- 创建时间（RFC3339）
    updated_at TEXT NOT NULL              -- 更新时间（RFC3339）
);

-- 示例数据
INSERT INTO remote_sync_envs VALUES (
    'a1b2c3-uuid',
    '北京数据中心',
    'mqtt.bj.example.com',
    1883,
    'http://fileserver.bj.example.com:8080/assets/archives',
    'bj',
    '7999,8001,8002,8003',
    1000,
    30000,
    '2025-09-28T10:00:00Z',
    '2025-09-28T10:00:00Z'
);
```

#### remote_sync_sites（站点表）

```sql
CREATE TABLE remote_sync_sites (
    id TEXT PRIMARY KEY,                  -- UUID
    env_id TEXT NOT NULL,                 -- 关联环境 ID
    name TEXT NOT NULL,                   -- 站点名称
    location TEXT,                        -- 站点地区
    http_host TEXT,                       -- HTTP 访问地址
    dbnums TEXT,                          -- 同步的数据库编号（逗号分隔）
    notes TEXT,                           -- 备注
    created_at TEXT NOT NULL,             -- 创建时间（RFC3339）
    updated_at TEXT NOT NULL,             -- 更新时间（RFC3339）
    FOREIGN KEY(env_id) REFERENCES remote_sync_envs(id) ON DELETE CASCADE
);

-- 示例数据
INSERT INTO remote_sync_sites VALUES (
    'd4e5f6-uuid',
    'a1b2c3-uuid',
    '上海站点-A',
    'sh',
    'http://shanghai-site-a.example.com:8080',
    '8010,8011,8012',
    '主要业务站点',
    '2025-09-28T10:05:00Z',
    '2025-09-28T10:05:00Z'
);
```

### Rust 数据结构

#### RemoteSyncEnv

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSyncEnv {
    pub id: String,
    pub name: String,
    pub mqtt_host: Option<String>,
    pub mqtt_port: Option<u16>,
    pub file_server_host: Option<String>,
    pub location: Option<String>,
    pub location_dbs: Option<String>,        // "7999,8001,8002"
    pub reconnect_initial_ms: Option<u64>,
    pub reconnect_max_ms: Option<u64>,
    pub created_at: String,
    pub updated_at: String,
}
```

#### RemoteSyncSite

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSyncSite {
    pub id: String,
    pub env_id: String,
    pub name: String,
    pub location: Option<String>,
    pub http_host: Option<String>,
    pub dbnums: Option<String>,              // "8010,8011,8012"
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}
```

#### SyncControlState

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncControlState {
    pub is_running: bool,
    pub is_paused: bool,
    pub current_env: Option<String>,
    pub env_name: Option<String>,
    pub mqtt_connected: bool,
    pub watcher_active: bool,
    pub last_mqtt_connect_time: Option<SystemTime>,
    pub mqtt_reconnect_count: u32,
    pub total_synced: u64,
    pub total_failed: u64,
    pub pending_count: u32,
    pub queue_size: u32,
    pub sync_rate_mbps: f64,
    pub avg_sync_time_ms: u64,
    pub last_sync_time: Option<SystemTime>,
    pub started_at: Option<SystemTime>,
    pub uptime_seconds: u64,
}
```

#### SyncTask

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTask {
    pub id: String,
    pub file_path: String,
    pub file_size: u64,
    pub status: SyncTaskStatus,
    pub priority: u8,
    pub retry_count: u32,
    pub created_at: SystemTime,
    pub started_at: Option<SystemTime>,
    pub completed_at: Option<SystemTime>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncTaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}
```

---

## API 完整文档

### 基础响应格式

**成功响应：**
```json
{
  "status": "success",
  "item": { ... },        // 单个资源
  "items": [ ... ],       // 资源列表
  "message": "操作成功"
}
```

**错误响应：**
```json
{
  "status": "error",
  "message": "错误描述"
}
```

### 环境管理 API

#### 1. 获取环境列表

```http
GET /api/remote-sync/envs
```

**响应：**
```json
{
  "status": "success",
  "items": [
    {
      "id": "uuid",
      "name": "北京数据中心",
      "mqtt_host": "mqtt.bj.example.com",
      "mqtt_port": 1883,
      "file_server_host": "http://fileserver.bj.example.com:8080",
      "location": "bj",
      "location_dbs": "7999,8001,8002",
      "reconnect_initial_ms": 1000,
      "reconnect_max_ms": 30000,
      "created_at": "2025-09-28T10:00:00Z",
      "updated_at": "2025-09-28T10:00:00Z"
    }
  ]
}
```

#### 2. 创建环境

```http
POST /api/remote-sync/envs
Content-Type: application/json

{
  "name": "北京数据中心",
  "mqtt_host": "mqtt.bj.example.com",
  "mqtt_port": 1883,
  "file_server_host": "http://fileserver.bj.example.com:8080",
  "location": "bj",
  "location_dbs": "7999,8001,8002",
  "reconnect_initial_ms": 1000,
  "reconnect_max_ms": 30000
}
```

**响应：**
```json
{
  "status": "success",
  "id": "newly-created-uuid"
}
```

#### 3. 获取单个环境

```http
GET /api/remote-sync/envs/{id}
```

**响应：**
```json
{
  "status": "success",
  "item": { ... }
}
```

#### 4. 更新环境

```http
PUT /api/remote-sync/envs/{id}
Content-Type: application/json

{
  "name": "北京数据中心（更新）",
  "mqtt_host": "mqtt2.bj.example.com",
  ...
}
```

#### 5. 删除环境

```http
DELETE /api/remote-sync/envs/{id}
```

**注意：** 会级联删除关联的所有站点。

#### 6. 应用环境配置

```http
POST /api/remote-sync/envs/{id}/apply
```

**作用：** 将环境配置写入 `DbOption.toml` 文件。

**响应：**
```json
{
  "status": "success",
  "message": "已写入 DbOption.toml",
  "hint": "如需启用 watcher/MQTT，请在配置中打开 sync_live 或重启 CLI 任务。"
}
```

#### 7. 激活环境（启动同步）

```http
POST /api/remote-sync/envs/{id}/activate
```

**作用：**
1. 写入 DbOption.toml
2. 停止当前运行时
3. 启动新的 watcher + MQTT 订阅

**响应：**
```json
{
  "status": "success",
  "message": "已写入 DbOption.toml 并启动 watcher + MQTT 订阅",
  "env_id": "uuid"
}
```

### 站点管理 API

#### 8. 获取环境下的站点列表

```http
GET /api/remote-sync/envs/{env_id}/sites
```

**响应：**
```json
{
  "status": "success",
  "items": [
    {
      "id": "uuid",
      "env_id": "parent-env-uuid",
      "name": "上海站点-A",
      "location": "sh",
      "http_host": "http://shanghai-site-a.example.com:8080",
      "dbnums": "8010,8011,8012",
      "notes": "主要业务站点",
      "created_at": "2025-09-28T10:05:00Z",
      "updated_at": "2025-09-28T10:05:00Z"
    }
  ]
}
```

#### 9. 创建站点

```http
POST /api/remote-sync/envs/{env_id}/sites
Content-Type: application/json

{
  "name": "上海站点-A",
  "location": "sh",
  "http_host": "http://shanghai-site-a.example.com:8080",
  "dbnums": "8010,8011,8012",
  "notes": "主要业务站点"
}
```

#### 10. 更新站点

```http
PUT /api/remote-sync/sites/{site_id}
Content-Type: application/json

{
  "name": "上海站点-A（更新）",
  ...
}
```

#### 11. 删除站点

```http
DELETE /api/remote-sync/sites/{site_id}
```

### 运行时控制 API

#### 12. 停止运行时

```http
POST /api/remote-sync/runtime/stop
```

**作用：** 停止文件监控和 MQTT 订阅。

#### 13. 获取运行时状态

```http
GET /api/remote-sync/runtime/status
```

**响应：**
```json
{
  "status": "success",
  "env_id": "current-uuid",
  "mqtt_connected": true,
  "watcher_active": true
}
```

#### 14. 获取运行时配置

```http
GET /api/remote-sync/runtime/config
```

**响应：** 返回当前 DbOption.toml 的相关配置。

#### 15. 从 DbOption 导入环境

```http
POST /api/remote-sync/envs/import-from-dboption
```

**作用：** 读取 DbOption.toml 并创建对应的环境记录。

### 同步控制 API

#### 16. 启动同步服务

```http
POST /api/sync/start
Content-Type: application/json

{
  "env_id": "uuid"
}
```

#### 17. 停止同步服务

```http
POST /api/sync/stop
```

#### 18. 重启同步服务

```http
POST /api/sync/restart
```

#### 19. 暂停同步

```http
POST /api/sync/pause
```

#### 20. 恢复同步

```http
POST /api/sync/resume
```

#### 21. 获取同步状态

```http
GET /api/sync/status
```

**响应：**
```json
{
  "status": "success",
  "state": {
    "is_running": true,
    "is_paused": false,
    "current_env": "uuid",
    "env_name": "北京数据中心",
    "mqtt_connected": true,
    "watcher_active": true,
    "total_synced": 1523,
    "total_failed": 3,
    "pending_count": 5,
    "queue_size": 10,
    "sync_rate_mbps": 12.5,
    "avg_sync_time_ms": 350,
    "uptime_seconds": 86400
  },
  "config": { ... },
  "mqtt_server": "mqtt.bj.example.com:1883",
  "queue_length": 10,
  "running_tasks": 3,
  "history_count": 1523
}
```

#### 22. 事件流（SSE）

```http
GET /api/sync/events
Accept: text/event-stream
```

**响应：** Server-Sent Events 流

```
data: {"type":"Started","data":{"env_id":"uuid","timestamp":"..."}}

data: {"type":"SyncStarted","data":{"file_path":"...","size":1024,"timestamp":"..."}}

data: {"type":"SyncCompleted","data":{"file_path":"...","duration_ms":350,"timestamp":"..."}}
```

#### 23. 获取性能指标

```http
GET /api/sync/metrics
```

**响应：**
```json
{
  "status": "success",
  "metrics": {
    "sync_rate_mbps": 12.5,
    "cpu_usage": 25.3,
    "memory_usage": 45.6,
    "active_tasks": 3,
    "pending_tasks": 7,
    "completed_total": 1523,
    "failed_total": 3
  }
}
```

#### 24. 获取任务队列

```http
GET /api/sync/queue
```

#### 25. 清空任务队列

```http
POST /api/sync/queue/clear
```

#### 26. 获取同步配置

```http
GET /api/sync/config
```

#### 27. 更新同步配置

```http
PUT /api/sync/config
Content-Type: application/json

{
  "auto_retry": true,
  "max_retries": 3,
  "retry_delay_ms": 5000,
  "max_concurrent_syncs": 5,
  "batch_size": 10,
  "sync_interval_ms": 1000
}
```

#### 28. 测试连接

```http
POST /api/sync/test
Content-Type: application/json

{
  "mqtt_host": "mqtt.example.com",
  "mqtt_port": 1883
}
```

#### 29. 添加同步任务

```http
POST /api/sync/task
Content-Type: application/json

{
  "file_path": "/path/to/file.db",
  "file_size": 1024000,
  "priority": 5
}
```

#### 30. 取消同步任务

```http
POST /api/sync/task/{task_id}/cancel
```

#### 31. 获取同步历史

```http
GET /api/sync/history?limit=100&offset=0
```

#### 32. 启动 MQTT 服务

```http
POST /api/sync/mqtt/start
```

#### 33. 停止 MQTT 服务

```http
POST /api/sync/mqtt/stop
```

#### 34. MQTT 服务状态

```http
GET /api/sync/mqtt/status
```

---

## 同步机制

### MQTT 实时同步

#### 消息格式

**Topic 规范：**
```
e3d/{location}/{dbnum}/update
```

**示例：**
- `e3d/bj/7999/update`
- `e3d/sh/8010/update`

**消息体（JSON）：**
```json
{
  "event": "file_updated",
  "location": "bj",
  "dbnum": 7999,
  "file_path": "/data/e3d/7999.db",
  "file_size": 10240000,
  "timestamp": "2025-09-28T10:30:00Z",
  "checksum": "md5:abc123..."
}
```

#### QoS 级别

- **QoS 2 (Exactly Once)** - 确保消息不重复不丢失
- **Retain Flag** - 保留最后一条消息供新订阅者获取

#### 断线重连策略

**指数退避算法：**
```rust
let delay = min(
    initial_delay * (2 ^ retry_count),
    max_delay
);
```

**参数：**
- `reconnect_initial_ms`: 1000 (1秒)
- `reconnect_max_ms`: 30000 (30秒)

**示例序列：**
```
尝试 1: 1秒后重连
尝试 2: 2秒后重连
尝试 3: 4秒后重连
尝试 4: 8秒后重连
尝试 5: 16秒后重连
尝试 6: 30秒后重连 (达到上限)
尝试 7+: 30秒后重连 (持续)
```

### 文件监控同步

#### 监控机制

**Linux (inotify)：**
- IN_MODIFY
- IN_CREATE
- IN_MOVED_TO

**macOS (FSEvents)：**
- kFSEventStreamEventFlagItemModified
- kFSEventStreamEventFlagItemCreated

**Windows (ReadDirectoryChangesW)：**
- FILE_NOTIFY_CHANGE_LAST_WRITE
- FILE_NOTIFY_CHANGE_FILE_NAME

#### 同步流程

```
1. 文件变更检测
   ↓
2. 添加到任务队列（带优先级）
   ↓
3. 等待调度（考虑并发限制）
   ↓
4. 执行同步（HTTP 下载 + 校验）
   ↓
5. 更新本地文件
   ↓
6. 发送完成事件
```

#### 冲突处理

**策略：**
- 优先级高的任务先执行
- 相同文件的任务合并（去重）
- 失败任务自动重试（最多 3 次）
- 重试间隔指数增长

---

## UI 改造方案

### 🎯 目标

1. 用现代化的 React UI 替换 Alpine.js 界面
2. 保持对现有 API 的兼容性
3. 提升用户体验和代码可维护性
4. 实现平滑迁移，不影响现有功能

### 📋 改造策略

#### 方案：数据适配层

在前端新增一个适配层，将现有 API 数据映射为新 UI 期望的格式。

```
现有 API
   ↓
适配层 (adapter.ts)
   ↓
新 UI 组件
```

### 🔄 数据映射关系

#### RemoteSyncEnv → CollaborationGroup

```typescript
// 适配器函数
function envToGroup(env: RemoteSyncEnv): CollaborationGroup {
  return {
    id: env.id,
    name: env.name,
    description: `${env.location || '未知地区'} - MQTT: ${env.mqtt_host || '未配置'}`,
    group_type: "DataSync",  // 固定为数据同步类型
    site_ids: [],  // 需要额外查询站点列表
    primary_site_id: undefined,
    shared_config: {
      mqtt_host: env.mqtt_host,
      mqtt_port: env.mqtt_port,
      file_server_host: env.file_server_host,
      location: env.location,
      location_dbs: env.location_dbs,
    },
    sync_strategy: {
      mode: "OneWay",  // MQTT 是单向推送
      interval_seconds: 60,  // MQTT 是实时的，这里设置一个默认值
      auto_sync: true,
      conflict_resolution: "LatestWins",
    },
    status: "Active",  // 根据运行时状态判断
    creator: "system",
    created_at: env.created_at,
    updated_at: env.updated_at,
    tags: {
      reconnect_initial_ms: env.reconnect_initial_ms,
      reconnect_max_ms: env.reconnect_max_ms,
    },
  };
}
```

#### RemoteSyncSite → Site (in Group)

```typescript
function siteToGroupSite(site: RemoteSyncSite): any {
  return {
    id: site.id,
    name: site.name,
    location: site.location,
    http_host: site.http_host,
    dbnums: site.dbnums?.split(',').map(s => s.trim()),
    notes: site.notes,
    created_at: site.created_at,
    updated_at: site.updated_at,
  };
}
```

#### SyncControlState → CollaborationGroup.status

```typescript
function syncStateToStatus(state: SyncControlState): CollaborationGroupStatus {
  if (!state.is_running) return "Paused";
  if (state.total_failed > state.total_synced * 0.1) return "Error";  // 失败率 > 10%
  if (state.pending_count > 0 || state.queue_size > 0) return "Syncing";
  return "Active";
}
```

### 📝 UI 组件修改建议

#### 1. 创建协同组对话框（create-group-dialog.tsx）

**当前问题：**
- 表单字段与后端不匹配
- 缺少 MQTT 配置字段
- 站点选择逻辑需要调整

**修改建议：**

```typescript
// 新增字段映射
interface EnvFormData {
  // 基本信息
  name: string;
  description?: string;

  // MQTT 配置
  mqtt_host?: string;
  mqtt_port?: number;

  // 文件服务器
  file_server_host?: string;

  // 地区配置
  location?: string;
  location_dbs?: string;  // 逗号分隔

  // 重连策略
  reconnect_initial_ms?: number;
  reconnect_max_ms?: number;

  // 创建者
  creator: string;
}

// 提交函数修改
const handleSubmit = async () => {
  // 调用现有 API 而不是新设计的 API
  const result = await fetch('/api/remote-sync/envs', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      name: formData.name,
      mqtt_host: formData.mqtt_host,
      mqtt_port: formData.mqtt_port,
      file_server_host: formData.file_server_host,
      location: formData.location,
      location_dbs: formData.location_dbs,
      reconnect_initial_ms: formData.reconnect_initial_ms || 1000,
      reconnect_max_ms: formData.reconnect_max_ms || 30000,
    }),
  });

  const data = await result.json();

  // 将返回的环境转换为 CollaborationGroup 格式
  const group = envToGroup({
    id: data.id,
    name: formData.name,
    ...formData,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
  });

  onSuccess(group);
};
```

**UI 步骤调整：**

**步骤 1：基本信息**
- 环境名称 *
- 描述
- 地区标识（location）
- 创建者

**步骤 2：MQTT 配置**
- MQTT 主机 *
- MQTT 端口（默认 1883）
- 重连初始间隔（默认 1000ms）
- 重连最大间隔（默认 30000ms）

**步骤 3：文件服务与数据库**
- 文件服务器地址
- 负责的数据库编号（逗号分隔）
- 预览配置

#### 2. 站点选择器（site-selector.tsx）

**问题：**
- 站点数据结构不同
- 需要关联到环境而不是协同组

**修改建议：**

```typescript
// 修改为加载环境下的站点
const loadSites = async (envId: string) => {
  const response = await fetch(`/api/remote-sync/envs/${envId}/sites`);
  const data = await response.json();
  return data.items || [];
};

// 添加创建站点的功能
const createSite = async (envId: string, siteData: any) => {
  const response = await fetch(`/api/remote-sync/envs/${envId}/sites`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(siteData),
  });
  return response.json();
};
```

#### 3. 协同组列表页（collaboration/page.tsx）

**修改建议：**

```typescript
// 修改 API 调用
const loadGroups = async () => {
  // 加载环境列表
  const envResponse = await fetch('/api/remote-sync/envs');
  const envData = await envResponse.json();

  // 加载运行时状态
  const statusResponse = await fetch('/api/sync/status');
  const statusData = await statusResponse.json();

  // 转换为 CollaborationGroup 格式
  const groups = (envData.items || []).map((env: any) => {
    const group = envToGroup(env);
    // 根据运行时状态更新 status
    if (statusData.state?.current_env === env.id) {
      group.status = syncStateToStatus(statusData.state);
    } else {
      group.status = "Paused";
    }
    return group;
  });

  setGroups(groups);
};
```

#### 4. 协同组详情页（collaboration/[id]/page.tsx）

**修改建议：**

```typescript
const loadGroupData = async () => {
  // 加载环境详情
  const envResponse = await fetch(`/api/remote-sync/envs/${groupId}`);
  const envData = await envResponse.json();

  // 加载站点列表
  const sitesResponse = await fetch(`/api/remote-sync/envs/${groupId}/sites`);
  const sitesData = await sitesResponse.json();

  // 加载同步状态
  const statusResponse = await fetch('/api/sync/status');
  const statusData = await statusResponse.json();

  // 组装数据
  const group = envToGroup(envData.item);
  group.site_ids = (sitesData.items || []).map((s: any) => s.id);
  group.status = syncStateToStatus(statusData.state);

  setGroup(group);
};

// 修改同步操作
const handleSync = async () => {
  // 调用激活环境 API
  const response = await fetch(`/api/remote-sync/envs/${groupId}/activate`, {
    method: 'POST',
  });
  const data = await response.json();

  if (data.status === 'success') {
    // 刷新状态
    await loadGroupData();
  }
};
```

### 🔧 创建适配器模块

**新建文件：** `lib/api/collaboration-adapter.ts`

```typescript
import type { CollaborationGroup, RemoteSite, SyncRecord } from "@/types/collaboration";

// 现有 API 的类型定义
interface RemoteSyncEnv {
  id: string;
  name: string;
  mqtt_host?: string;
  mqtt_port?: number;
  file_server_host?: string;
  location?: string;
  location_dbs?: string;
  reconnect_initial_ms?: number;
  reconnect_max_ms?: number;
  created_at: string;
  updated_at: string;
}

interface RemoteSyncSite {
  id: string;
  env_id: string;
  name: string;
  location?: string;
  http_host?: string;
  dbnums?: string;
  notes?: string;
  created_at: string;
  updated_at: string;
}

interface SyncControlState {
  is_running: boolean;
  is_paused: boolean;
  current_env?: string;
  env_name?: string;
  mqtt_connected: boolean;
  watcher_active: boolean;
  total_synced: number;
  total_failed: number;
  pending_count: number;
  queue_size: number;
  sync_rate_mbps: number;
}

// 适配函数
export function envToGroup(env: RemoteSyncEnv, sites: RemoteSyncSite[] = []): CollaborationGroup {
  return {
    id: env.id,
    name: env.name,
    description: `${env.location || '未知地区'} - MQTT: ${env.mqtt_host || '未配置'}`,
    group_type: "DataSync",
    site_ids: sites.map(s => s.id),
    primary_site_id: undefined,
    shared_config: {
      mqtt_host: env.mqtt_host,
      mqtt_port: env.mqtt_port,
      file_server_host: env.file_server_host,
      location: env.location,
      location_dbs: env.location_dbs,
    },
    sync_strategy: {
      mode: "OneWay",
      interval_seconds: 60,
      auto_sync: true,
      conflict_resolution: "LatestWins",
    },
    status: "Active",
    creator: "system",
    created_at: env.created_at,
    updated_at: env.updated_at,
    tags: {
      reconnect_initial_ms: env.reconnect_initial_ms,
      reconnect_max_ms: env.reconnect_max_ms,
    },
  };
}

export function syncStateToStatus(state: SyncControlState) {
  if (!state.is_running) return "Paused";
  if (state.total_failed > state.total_synced * 0.1) return "Error";
  if (state.pending_count > 0 || state.queue_size > 0) return "Syncing";
  return "Active";
}

export function siteToRemoteSite(site: RemoteSyncSite): RemoteSite {
  return {
    id: site.id,
    name: site.name,
    api_url: site.http_host || "",
    connection_status: "Connected",
    latency_ms: undefined,
  };
}

// 适配后的 API 调用函数
export async function fetchCollaborationGroupsAdapter() {
  const [envResponse, statusResponse] = await Promise.all([
    fetch('/api/remote-sync/envs'),
    fetch('/api/sync/status').catch(() => ({ json: async () => ({ state: {} }) })),
  ]);

  const envData = await envResponse.json();
  const statusData = await statusResponse.json();

  const groups: CollaborationGroup[] = [];
  for (const env of envData.items || []) {
    const sitesResponse = await fetch(`/api/remote-sync/envs/${env.id}/sites`);
    const sitesData = await sitesResponse.json();
    const sites = sitesData.items || [];

    const group = envToGroup(env, sites);
    if (statusData.state?.current_env === env.id) {
      group.status = syncStateToStatus(statusData.state);
    } else {
      group.status = "Paused";
    }
    groups.push(group);
  }

  return { items: groups, total: groups.length };
}

export async function createCollaborationGroupAdapter(payload: any) {
  const response = await fetch('/api/remote-sync/envs', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      name: payload.name,
      mqtt_host: payload.shared_config?.mqtt_host,
      mqtt_port: payload.shared_config?.mqtt_port,
      file_server_host: payload.shared_config?.file_server_host,
      location: payload.shared_config?.location,
      location_dbs: payload.shared_config?.location_dbs,
      reconnect_initial_ms: payload.tags?.reconnect_initial_ms || 1000,
      reconnect_max_ms: payload.tags?.reconnect_max_ms || 30000,
    }),
  });

  const data = await response.json();
  const env: RemoteSyncEnv = {
    id: data.id,
    name: payload.name,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    ...payload.shared_config,
  };

  return { status: "success", item: envToGroup(env) };
}
```

### 📦 修改后的文件清单

```
frontend/v0-aios-database-management/
├── lib/api/
│   └── collaboration-adapter.ts       ✅ 新建（适配层）
├── types/
│   └── collaboration.ts                ✅ 已有（可能需要微调）
├── app/collaboration/
│   ├── page.tsx                       🔧 修改（使用适配器）
│   └── [id]/page.tsx                  🔧 修改（使用适配器）
└── components/collaboration/
    ├── create-group-dialog.tsx        🔧 修改（调整表单字段）
    └── site-selector.tsx              🔧 修改（调整数据加载）
```

---

## 实施步骤

### 第 1 阶段：准备工作（1 天）

1. ✅ 创建适配器模块 `collaboration-adapter.ts`
2. ✅ 调整类型定义，增加现有 API 的类型
3. ✅ 编写适配函数的单元测试

### 第 2 阶段：UI 组件改造（2-3 天）

4. 🔧 修改创建对话框
   - 调整表单字段为 MQTT 配置
   - 修改提交逻辑使用适配器
   - 测试创建流程

5. 🔧 修改列表页
   - 使用适配器加载数据
   - 调整状态显示逻辑
   - 测试筛选和搜索

6. 🔧 修改详情页
   - 使用适配器加载数据
   - 调整同步操作为激活环境
   - 添加站点管理功能

7. 🔧 修改站点选择器
   - 调整为环境下的站点管理
   - 添加创建站点功能
   - 测试选择逻辑

### 第 3 阶段：功能增强（1-2 天）

8. ✨ 添加实时状态更新（轮询或 SSE）
9. ✨ 添加同步历史记录查看
10. ✨ 添加性能监控图表
11. ✨ 添加错误告警提示

### 第 4 阶段：测试和优化（1-2 天）

12. 🧪 端到端测试
13. 🐛 Bug 修复
14. 📝 文档更新
15. 🚀 部署上线

---

## 最佳实践

### 1. 错误处理

```typescript
try {
  const result = await fetchWithTimeout('/api/remote-sync/envs', 5000);
  // 处理结果
} catch (error) {
  if (error instanceof TimeoutError) {
    toast.error('请求超时，请检查网络连接');
  } else if (error instanceof NetworkError) {
    toast.error('网络错误，请稍后重试');
  } else {
    toast.error(`操作失败: ${error.message}`);
  }
  console.error('详细错误:', error);
}
```

### 2. 加载状态

```typescript
const [loading, setLoading] = useState(false);

const loadData = async () => {
  setLoading(true);
  try {
    const data = await fetchData();
    setData(data);
  } finally {
    setLoading(false);  // 确保在 finally 中重置
  }
};
```

### 3. 防抖和节流

```typescript
import { debounce } from 'lodash-es';

// 搜索输入防抖
const debouncedSearch = useMemo(
  () => debounce((query: string) => {
    fetchSearchResults(query);
  }, 300),
  []
);
```

### 4. 实时更新

```typescript
// 使用 SSE 获取实时事件
useEffect(() => {
  const eventSource = new EventSource('/api/sync/events');

  eventSource.onmessage = (event) => {
    const data = JSON.parse(event.data);
    handleSyncEvent(data);
  };

  eventSource.onerror = () => {
    console.error('SSE connection error');
    eventSource.close();
  };

  return () => {
    eventSource.close();
  };
}, []);
```

### 5. 缓存策略

```typescript
// 使用 SWR 或 React Query
import useSWR from 'swr';

const { data, error, mutate } = useSWR(
  '/api/remote-sync/envs',
  fetcher,
  {
    refreshInterval: 30000,  // 30秒自动刷新
    revalidateOnFocus: true,
    dedupingInterval: 5000,
  }
);
```

---

## 总结

本技术指南提供了：

1. ✅ **完整的系统架构**：前后端分层、数据流向
2. ✅ **详细的 API 文档**：34 个接口的完整说明
3. ✅ **数据模型定义**：表结构、Rust 结构体、类型映射
4. ✅ **同步机制说明**：MQTT + 文件监控的工作原理
5. ✅ **UI 改造方案**：适配器模式、组件修改建议
6. ✅ **实施步骤**：分阶段的详细计划
7. ✅ **最佳实践**：错误处理、状态管理、性能优化

**核心策略：**
- 保留现有后端的技术优势（MQTT 实时同步）
- 用适配器层桥接新旧 API
- 提升前端体验和代码质量
- 平滑过渡，风险可控

现在可以开始实施改造了！ 🚀