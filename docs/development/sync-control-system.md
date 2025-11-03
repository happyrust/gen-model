# 异步同步控制系统文档

## 概述

本文档描述了 gen-model 项目中的异步同步控制系统，该系统提供了实时监控、任务调度和 MQTT 集成功能，使用户能够通过 Web UI 控制和监控数据同步过程。

## 1. 系统架构

### 1.1 整体架构

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Web UI    │────▶│ Sync Control│────▶│    MQTT     │
│  (Frontend) │     │   Handlers  │     │   Server    │
└─────────────┘     └─────────────┘     └─────────────┘
       │                    │                    │
       ▼                    ▼                    ▼
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Polling   │     │ Control     │     │   Event     │
│   Events    │     │   Center    │     │   Stream    │
└─────────────┘     └─────────────┘     └─────────────┘
```

### 1.2 核心组件

- **sync_control_center.rs**: 同步控制中心，管理全局状态和任务队列
- **sync_control_handlers.rs**: HTTP 处理器，提供 API 接口
- **sync_control_template.rs**: 前端模板，提供用户界面
- **rumqttd-server/**: 独立的 MQTT 服务器项目

## 2. 功能模块

### 2.1 同步控制中心

#### 2.1.1 核心数据结构

```rust
pub struct SyncControlCenter {
    /// 同步任务队列
    pub task_queue: Arc<Mutex<Vec<SyncTask>>>,
    
    /// MQTT 连接状态
    pub mqtt_connected: Arc<AtomicBool>,
    
    /// 文件监控器映射
    pub file_watchers: Arc<Mutex<HashMap<String, FileWatcher>>>,
    
    /// 活跃的同步会话
    pub active_sessions: Arc<Mutex<HashMap<String, SyncSession>>>,
}

pub struct SyncTask {
    pub id: String,
    pub name: String,
    pub task_type: SyncTaskType,
    pub status: SyncStatus,
    pub config: SyncConfig,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub progress: f32,
    pub logs: Vec<LogEntry>,
}

pub enum SyncTaskType {
    FileSync,          // 文件同步
    DatabaseSync,      // 数据库同步
    IncrementalSync,   // 增量同步
    FullSync,          // 全量同步
}
```

#### 2.1.2 事件系统

```rust
// 全局事件广播器
pub static SYNC_EVENT_TX: Lazy<broadcast::Sender<SyncEvent>> = Lazy::new(|| {
    let (tx, _) = broadcast::channel(1000);
    tx
});

pub enum SyncEvent {
    TaskQueued { task_id: String },
    TaskStarted { task_id: String },
    TaskProgress { task_id: String, progress: f32 },
    TaskCompleted { task_id: String },
    TaskFailed { task_id: String, error: String },
    MqttConnected,
    MqttDisconnected,
    FileChanged { path: String, change_type: String },
}
```

### 2.2 MQTT 集成

#### 2.2.1 连接管理

```rust
pub async fn connect_mqtt(config: MqttConfig) -> Result<MqttClient, Error> {
    let mut mqttoptions = MqttOptions::new(
        config.client_id,
        config.host,
        config.port,
    );
    
    mqttoptions.set_keep_alive(Duration::from_secs(30));
    mqttoptions.set_clean_session(true);
    
    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);
    
    // 订阅主题
    client.subscribe("sync/+/status", QoS::AtMostOnce).await?;
    client.subscribe("sync/+/progress", QoS::AtMostOnce).await?;
    client.subscribe("file/+/changed", QoS::AtMostOnce).await?;
    
    Ok(client)
}
```

#### 2.2.2 消息处理

```rust
pub async fn handle_mqtt_message(message: Publish) {
    match message.topic.as_str() {
        topic if topic.starts_with("sync/") => {
            handle_sync_message(message).await;
        }
        topic if topic.starts_with("file/") => {
            handle_file_message(message).await;
        }
        _ => {}
    }
}
```

### 2.3 文件监控

#### 2.3.1 监控器配置

```rust
pub struct FileWatcher {
    pub id: String,
    pub path: PathBuf,
    pub recursive: bool,
    pub filters: Vec<String>,
    pub debounce_ms: u64,
    pub watcher: RecommendedWatcher,
}
```

#### 2.3.2 变更检测

```rust
pub fn start_file_watcher(path: &Path, recursive: bool) -> Result<FileWatcher> {
    let (tx, rx) = channel();
    
    let mut watcher = RecommendedWatcher::new(
        tx,
        Config::default()
            .with_poll_interval(Duration::from_secs(2))
    )?;
    
    if recursive {
        watcher.watch(path, RecursiveMode::Recursive)?;
    } else {
        watcher.watch(path, RecursiveMode::NonRecursive)?;
    }
    
    // 处理文件变更事件
    tokio::spawn(async move {
        while let Ok(event) = rx.recv() {
            handle_file_event(event).await;
        }
    });
    
    Ok(watcher)
}
```

## 3. API 接口

### 3.1 同步服务控制

#### 启动同步服务

**请求：**
```http
POST /api/sync/start
Content-Type: application/json

{
    "sync_type": "incremental",
    "target_path": "/data/projects/sample",
    "mqtt_config": {
        "enabled": true,
        "broker_url": "mqtt://localhost:1883",
        "client_id": "sync_client_001"
    },
    "watch_config": {
        "enabled": true,
        "recursive": true,
        "debounce_ms": 500
    }
}
```

**响应：**
```json
{
    "task_id": "sync_20240119_103000",
    "status": "started",
    "message": "同步服务已启动"
}
```

#### 停止同步服务

```http
POST /api/sync/stop

{
    "task_id": "sync_20240119_103000"
}
```

### 3.2 状态查询

#### 获取同步状态

```http
GET /api/sync/status
```

**响应：**
```json
{
    "mqtt_connected": true,
    "active_tasks": 3,
    "completed_tasks": 15,
    "failed_tasks": 2,
    "file_watchers": 5,
    "last_sync_time": "2024-01-19T10:30:00Z",
    "tasks": [
        {
            "id": "sync_001",
            "name": "项目数据同步",
            "status": "running",
            "progress": 45.5,
            "started_at": "2024-01-19T10:00:00Z"
        }
    ]
}
```

### 3.3 事件轮询

#### 获取最新事件

```http
GET /api/sync/events/poll?last_event_id=12345
```

**响应：**
```json
{
    "events": [
        {
            "id": 12346,
            "type": "task_progress",
            "data": {
                "task_id": "sync_001",
                "progress": 50.0
            },
            "timestamp": "2024-01-19T10:31:00Z"
        },
        {
            "id": 12347,
            "type": "file_changed",
            "data": {
                "path": "/data/projects/sample/config.toml",
                "change_type": "modified"
            },
            "timestamp": "2024-01-19T10:31:05Z"
        }
    ],
    "last_event_id": 12347
}
```

### 3.4 任务管理

#### 创建同步任务

```http
POST /api/sync/tasks

{
    "name": "数据库增量同步",
    "type": "incremental",
    "config": {
        "source": "/data/source",
        "target": "/data/target",
        "filters": ["*.db", "*.sqlite"],
        "interval_seconds": 60
    }
}
```

#### 查询任务列表

```http
GET /api/sync/tasks?status=running&limit=10
```

## 4. 前端界面

### 4.1 控制面板

前端提供了一个实时监控面板，包含：

- **连接状态指示器**：显示 MQTT 连接状态
- **任务列表**：展示所有同步任务及其进度
- **事件日志**：实时显示系统事件
- **控制按钮**：启动/停止同步服务

### 4.2 实时更新机制

由于 SSE 版本冲突，系统使用轮询机制实现实时更新：

```javascript
class SyncMonitor {
    constructor() {
        this.lastEventId = 0;
        this.pollInterval = 2000; // 2秒轮询一次
    }
    
    async startPolling() {
        setInterval(async () => {
            const events = await this.fetchEvents();
            this.processEvents(events);
        }, this.pollInterval);
    }
    
    async fetchEvents() {
        const response = await fetch(
            `/api/sync/events/poll?last_event_id=${this.lastEventId}`
        );
        const data = await response.json();
        this.lastEventId = data.last_event_id;
        return data.events;
    }
    
    processEvents(events) {
        events.forEach(event => {
            switch(event.type) {
                case 'task_progress':
                    this.updateProgress(event.data);
                    break;
                case 'file_changed':
                    this.handleFileChange(event.data);
                    break;
                // ... 其他事件处理
            }
        });
    }
}
```

## 5. 配置说明

### 5.1 MQTT 服务器配置

```toml
# rumqttd.toml
[broker]
id = 0

[[broker.servers]]
name = "sync_server"
listens = ["0.0.0.0:1883"]

[broker.servers.connections]
connection_timeout_ms = 60000
max_payload_size = 268435456
max_inflight_count = 100
auth = { allow_anonymous = true }

[[broker.servers.alerts]]
name = "console"
config = { filename = "rumqttd.log" }
```

### 5.2 同步配置

```toml
# sync_config.toml
[sync]
max_concurrent_tasks = 10
default_interval_seconds = 60
retry_attempts = 3
retry_delay_seconds = 5

[mqtt]
enabled = true
broker_url = "mqtt://localhost:1883"
client_id_prefix = "sync_client"
keep_alive_seconds = 30

[file_watch]
enabled = true
debounce_ms = 500
max_watchers = 100
ignore_patterns = [
    "*.tmp",
    "*.log",
    ".git/**"
]
```

## 6. 错误处理

### 6.1 错误类型

```rust
pub enum SyncError {
    MqttConnectionFailed(String),
    FileWatcherError(String),
    TaskExecutionError(String),
    ConfigurationError(String),
    DatabaseError(String),
}
```

### 6.2 错误恢复策略

| 错误类型 | 恢复策略 | 重试次数 |
|---------|---------|----------|
| MQTT 连接失败 | 指数退避重连 | 无限 |
| 文件监控失败 | 重新创建监控器 | 3次 |
| 任务执行失败 | 根据配置重试 | 可配置 |
| 配置错误 | 返回错误，等待修正 | 不重试 |

### 6.3 日志记录

所有错误都会记录到日志系统：

```rust
pub fn log_error(error: &SyncError, context: &str) {
    error!("同步错误 [{}]: {:?}", context, error);
    
    // 发送事件通知
    let _ = SYNC_EVENT_TX.send(SyncEvent::Error {
        error: error.to_string(),
        context: context.to_string(),
    });
}
```

## 7. 性能优化

### 7.1 任务队列优化

- 使用优先队列管理任务
- 实现任务批处理机制
- 动态调整并发数

### 7.2 事件处理优化

- 事件去重和合并
- 批量发送事件更新
- 使用环形缓冲区存储事件

### 7.3 文件监控优化

- 使用 debounce 减少事件频率
- 忽略临时文件和日志文件
- 限制最大监控器数量

## 8. 监控指标

### 8.1 关键指标

- **同步延迟**：从变更发生到同步完成的时间
- **吞吐量**：每秒处理的文件数或数据量
- **错误率**：失败任务占总任务的比例
- **队列长度**：待处理任务数量
- **连接稳定性**：MQTT 连接断开次数

### 8.2 告警阈值

| 指标 | 警告阈值 | 严重阈值 |
|-----|---------|----------|
| 同步延迟 | > 5分钟 | > 15分钟 |
| 错误率 | > 5% | > 15% |
| 队列长度 | > 100 | > 500 |
| 连接断开 | > 5次/小时 | > 20次/小时 |

## 9. 部署建议

### 9.1 系统要求

- CPU: 2核以上
- 内存: 4GB以上
- 磁盘: SSD 推荐
- 网络: 低延迟网络环境

### 9.2 部署步骤

1. **编译 MQTT 服务器**
   ```bash
   cd rumqttd-server
   cargo build --release
   ./target/release/rumqttd -c rumqttd.toml
   ```

2. **启动主服务**
   ```bash
   cargo run --bin web_ui --features web_ui
   ```

3. **配置反向代理**（可选）
   ```nginx
   location /api/sync/ {
       proxy_pass http://localhost:8080;
       proxy_http_version 1.1;
       proxy_set_header Upgrade $http_upgrade;
       proxy_set_header Connection "upgrade";
   }
   ```

## 10. 故障排查

### 10.1 常见问题

| 问题 | 可能原因 | 解决方案 |
|-----|---------|----------|
| MQTT 无法连接 | 服务器未启动或端口被占用 | 检查服务状态和端口 |
| 文件变更未检测 | 权限问题或路径错误 | 检查文件权限和路径 |
| 同步任务卡住 | 死锁或资源不足 | 重启任务或增加资源 |
| 事件更新延迟 | 轮询间隔过长 | 减小轮询间隔 |

### 10.2 调试工具

- **日志查看**: `tail -f logs/sync.log`
- **MQTT 监控**: 使用 MQTT 客户端订阅 `sync/#`
- **API 测试**: 使用 Postman 或 curl 测试接口
- **性能分析**: 启用 profile feature 进行分析

## 附录

### A. 相关文件

- `/src/web_ui/sync_control_center.rs` - 同步控制中心
- `/src/web_ui/sync_control_handlers.rs` - API 处理器
- `/src/web_ui/sync_control_template.rs` - 前端模板
- `/rumqttd-server/` - MQTT 服务器
- `/static/sync_monitor.js` - 前端监控脚本

### B. 依赖项

- axum 0.7 - Web 框架
- rumqttc - MQTT 客户端
- tokio - 异步运行时
- notify - 文件监控
- serde - 序列化

### C. 相关文档

- [任务创建工作流程](./task-creation-workflow.md)
- [API 参考](./api-reference.md)
- [部署指南](./deployment-guide.md)

---

*最后更新：2024-01-19*
*版本：1.0.0*