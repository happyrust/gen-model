# WebUI 数据库管理系统开发文档

## 目录
1. [系统概述](#系统概述)
2. [架构设计](#架构设计)
3. [功能模块](#功能模块)
4. [API接口规范](#api接口规范)
5. [数据模型](#数据模型)
6. [部署指南](#部署指南)
7. [使用说明](#使用说明)

---

## 系统概述

### 项目背景
AIOS Database WebUI 是一个用于管理 PDMS 数据解析、3D 模型生成和增量更新的 Web 管理系统。系统提供了完整的数据库生命周期管理功能，包括数据库状态监控、批量操作、增量更新检测等。

### 技术栈
- **后端**: Rust + Axum Web Framework
- **数据库**: SurrealDB (主数据库) + SQLite (R-Tree空间索引)
- **前端**: HTML5 + TailwindCSS + Vanilla JavaScript
- **实时通信**: WebSocket (用于进度推送)

### 核心功能
1. **数据库连接管理** - 启动/停止/测试 SurrealDB 连接
2. **数据库状态监控** - 实时显示各数据库处理状态
3. **增量更新检测** - 监控文件变更并触发增量同步
4. **批量操作** - 支持批量解析、生成、更新等操作
5. **任务管理** - 创建、监控、取消各类处理任务

---

## 架构设计

### 系统架构图
```
┌─────────────────────────────────────────────────────┐
│                   WebUI Frontend                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐  │
│  │ Database │  │Incremental│  │  Database Status │  │
│  │ Connect  │  │  Update   │  │   Management     │  │
│  └─────┬────┘  └─────┬────┘  └────────┬─────────┘  │
└────────┼─────────────┼────────────────┼────────────┘
         │             │                │
         ▼             ▼                ▼
┌─────────────────────────────────────────────────────┐
│                  Axum Web Server                     │
│  ┌──────────────────────────────────────────────┐  │
│  │              REST API Layer                   │  │
│  └──────────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────────┐  │
│  │            Handler Functions                  │  │
│  └──────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────┘
         │                    │                │
         ▼                    ▼                ▼
┌──────────────┐    ┌──────────────┐   ┌─────────────┐
│  SurrealDB   │    │    SQLite    │   │   File      │
│   Database   │    │  R-Tree Index│   │   System    │
└──────────────┘    └──────────────┘   └─────────────┘
```

### 模块结构
```rust
src/web_ui/
├── mod.rs                          // 主模块定义和路由配置
├── handlers.rs                     // 基础请求处理器
├── database_status_handlers.rs    // 数据库状态管理
├── incremental_update_handlers.rs // 增量更新处理
├── db_startup_handlers.rs         // 数据库启动管理
├── db_startup_manager.rs          // 数据库进程管理
├── templates/                     // HTML模板
│   ├── database_status.html
│   ├── incremental_update.html
│   └── index.html
└── static/                        // 静态资源
    ├── database_status.js
    ├── incremental_update.js
    └── db_startup.js
```

---

## 功能模块

### 1. 数据库连接管理模块

#### 功能描述
管理 SurrealDB 数据库的生命周期，包括启动、停止、连接测试等。

#### 核心组件
- `DbStartupManager`: 数据库进程管理器
- `DbStartupStatus`: 启动状态枚举
- `DbInstanceInfo`: 实例信息结构体

#### 实现代码
```rust
// src/web_ui/db_startup_manager.rs
pub async fn start_database_with_progress(
    ip: String,
    port: u16,
    user: String,
    password: String,
    db_file: String,
) -> Result<u32, String> {
    // 1. 检查端口占用
    if check_port_in_use(&ip, port).await {
        kill_port_processes(port).await?;
    }

    // 2. 启动 SurrealDB
    let bind_addr = format!("0.0.0.0:{}", port);
    let child = Command::new("./surreal")
        .arg("start")
        .arg("--bind").arg(&bind_addr)
        .arg("--user").arg(&user)
        .arg("--pass").arg(&password)
        .arg(format!("file:{}", db_file))
        .spawn()?;

    // 3. 等待启动完成
    wait_for_database_ready(&ip, port).await?;

    Ok(child.id())
}
```

### 2. 数据库状态管理模块

#### 功能描述
展示所有数据库的处理状态，支持过滤、排序、批量操作。

#### 数据结构
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseStatus {
    pub db_num: u32,                    // 数据库编号
    pub db_name: String,                // 数据库名称
    pub module: String,                 // 模块类型
    pub parse_status: ProcessStatus,    // 解析状态
    pub model_status: ProcessStatus,    // 模型生成状态
    pub spatial_tree_status: ProcessStatus, // 空间树状态
    pub needs_update: bool,             // 是否需要更新
    pub last_parsed: Option<DateTime<Utc>>,
    pub last_generated: Option<DateTime<Utc>>,
    pub file_size: f64,                 // MB
    pub element_count: usize,
    pub triangle_count: usize,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProcessStatus {
    NotStarted,  // 未开始
    InProgress,  // 处理中
    Completed,   // 已完成
    Failed,      // 失败
    Outdated,    // 已过期
}
```

#### 前端表格实现
```javascript
// src/web_ui/static/database_status.js
class DatabaseStatusManager {
    async loadDatabases() {
        const params = new URLSearchParams({
            page: this.currentPage,
            page_size: this.pageSize,
            sort_by: this.sortField,
            order: this.sortOrder,
            ...this.filters
        });

        const response = await fetch(`/api/database/status?${params}`);
        const data = await response.json();

        this.renderTable(data.databases);
        this.updateStatistics(data.statistics);
        this.updatePagination(data.pagination);
    }

    renderTableRow(db) {
        return `
            <tr>
                <td>${db.db_num}</td>
                <td>${db.db_name}</td>
                <td>${this.getStatusBadge(db.parse_status)}</td>
                <td>${this.getStatusBadge(db.model_status)}</td>
                <td>${this.getStatusBadge(db.spatial_tree_status)}</td>
                <td>${db.needs_update ? '⚠️' : '✅'}</td>
                <td>${this.getActionButtons(db)}</td>
            </tr>
        `;
    }
}
```

### 3. 增量更新检测模块

#### 功能描述
监控部署站点的文件变更，自动检测并同步增量更新。

#### 核心功能
- 自动检测文件变更
- 计算增量大小
- 批量同步管理
- 同步历史记录

#### 实现方案
```rust
// src/web_ui/incremental_update_handlers.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncrementalUpdateInfo {
    pub site_id: String,
    pub site_name: String,
    pub last_sync_time: Option<DateTime<Utc>>,
    pub detection_status: UpdateDetectionStatus,
    pub pending_items: usize,
    pub synced_items: usize,
    pub changed_files: Vec<ChangedFile>,
    pub increment_size: u64,
    pub estimated_sync_time: u32,
}

pub async fn start_incremental_detection(
    Path(site_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    // 1. 扫描文件系统
    let changes = scan_file_changes(&site_id).await?;

    // 2. 计算增量
    let increment = calculate_increment(&changes);

    // 3. 更新状态
    update_site_status(&site_id, UpdateDetectionStatus::ChangesDetected);

    Ok(Json(json!({
        "success": true,
        "changes": changes.len(),
        "size": increment.size
    })))
}
```

---

## API接口规范

### 基础路由
```rust
// src/web_ui/mod.rs
pub fn create_router() -> Router {
    Router::new()
        // 页面路由
        .route("/", get(serve_index))
        .route("/database-status", get(serve_database_status_page))
        .route("/incremental", get(serve_incremental_update_page))

        // 数据库连接API
        .route("/api/database/startup/start", post(start_database_api))
        .route("/api/database/startup/stop", post(stop_database_api))
        .route("/api/database/startup/status", get(get_startup_status))

        // 数据库状态API
        .route("/api/database/status", get(get_all_database_status))
        .route("/api/database/:db_num/details", get(get_database_details))
        .route("/api/database/:db_num/parse", post(reparse_database))
        .route("/api/database/:db_num/generate", post(regenerate_model))
        .route("/api/database/batch", post(execute_batch_operation))

        // 增量更新API
        .route("/api/incremental/status", get(get_all_incremental_status))
        .route("/api/incremental/detect/:site_id", post(start_detection))
        .route("/api/incremental/sync/:site_id", post(start_sync))

        // 静态文件
        .nest_service("/static", ServeDir::new("src/web_ui/static"))
}
```

### API 接口文档

#### 1. 数据库状态查询
```http
GET /api/database/status?module=DESI&status=completed&page=1&page_size=20

Response:
{
    "success": true,
    "databases": [...],
    "pagination": {
        "total": 100,
        "page": 1,
        "page_size": 20
    },
    "statistics": {
        "total": 100,
        "parsed": 80,
        "generated": 75,
        "needs_update": 10,
        "failed": 5
    }
}
```

#### 2. 批量操作
```http
POST /api/database/batch
Content-Type: application/json

{
    "db_nums": [7999, 8001, 8002],
    "operation": "parse"  // parse|generate|update|clear
}

Response:
{
    "success": true,
    "task_id": "batch_parse_1234567890",
    "affected_databases": 3
}
```

#### 3. 增量检测
```http
POST /api/incremental/detect/site_001

Response:
{
    "success": true,
    "task_id": "detect_site_001_1234567890",
    "message": "已启动站点 site_001 的增量检测"
}
```

---

## 数据模型

### SurrealDB 表结构
```sql
-- 数据库状态表
DEFINE TABLE database_status SCHEMAFULL;
DEFINE FIELD db_num ON database_status TYPE int;
DEFINE FIELD db_name ON database_status TYPE string;
DEFINE FIELD module ON database_status TYPE string;
DEFINE FIELD parse_status ON database_status TYPE string;
DEFINE FIELD model_status ON database_status TYPE string;
DEFINE FIELD needs_update ON database_status TYPE bool;
DEFINE FIELD last_parsed ON database_status TYPE datetime;
DEFINE FIELD file_size ON database_status TYPE float;
DEFINE INDEX idx_db_num ON database_status COLUMNS db_num UNIQUE;

-- 增量更新记录表
DEFINE TABLE incremental_updates SCHEMAFULL;
DEFINE FIELD site_id ON incremental_updates TYPE string;
DEFINE FIELD changed_files ON incremental_updates TYPE array;
DEFINE FIELD sync_time ON incremental_updates TYPE datetime;
DEFINE FIELD status ON incremental_updates TYPE string;
```

### SQLite R-Tree 索引
```sql
-- 空间索引表
CREATE VIRTUAL TABLE aabb_rtree USING rtree(
    id,
    min_x, max_x,
    min_y, max_y,
    min_z, max_z
);

-- 元数据表
CREATE TABLE aabb_metadata (
    id INTEGER PRIMARY KEY,
    refno TEXT NOT NULL,
    db_num INTEGER,
    element_type TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
```

---

## 部署指南

### 环境要求
- Rust 1.70+
- SurrealDB 2.0+
- Node.js 16+ (仅用于前端构建)

### 编译步骤
```bash
# 1. 克隆项目
git clone https://github.com/your-repo/gen-model.git
cd gen-model

# 2. 编译 WebUI
cargo build --release --bin web_ui --features "web_ui,ws"

# 3. 启动服务
./target/release/web_ui

# 服务将在 http://localhost:8080 启动
```

### 配置文件
```toml
# DbOption.toml
[database]
v_ip = "localhost"
v_port = 8009
v_user = "root"
v_password = "1516"

[web_ui]
port = 8080
host = "0.0.0.0"

[sqlite_index]
enable_sqlite_rtree = true
sqlite_index_path = "aabb_cache.sqlite"
```

### Docker 部署
```dockerfile
# Dockerfile
FROM rust:1.70 as builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin web_ui --features "web_ui,ws"

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/web_ui /usr/local/bin/
COPY --from=builder /app/src/web_ui /app/src/web_ui
WORKDIR /app
EXPOSE 8080
CMD ["web_ui"]
```

```yaml
# docker-compose.yml
version: '3.8'
services:
  web_ui:
    build: .
    ports:
      - "8080:8080"
    volumes:
      - ./data:/app/data
      - ./DbOption.toml:/app/DbOption.toml
    environment:
      - RUST_LOG=info

  surrealdb:
    image: surrealdb/surrealdb:latest
    ports:
      - "8009:8009"
    volumes:
      - ./surrealdb-data:/data
    command: start file:/data/YCYK-E3D.rdb --user root --pass 1516 --bind 0.0.0.0:8009
```

---

## 使用说明

### 1. 首次启动

1. **访问 WebUI**
   ```
   http://localhost:8080
   ```

2. **配置数据库连接**
   - 点击"数据库连接"标签
   - 输入连接信息：
     - IP: localhost
     - 端口: 8009
     - 用户名: root
     - 密码: 1516
     - 数据库文件: YCYK-E3D.rdb

3. **启动数据库**
   - 点击"启动"按钮
   - 等待状态变为"运行中"

### 2. 数据库状态管理

1. **查看状态**
   - 访问 `/database-status`
   - 查看所有数据库的处理状态

2. **批量操作**
   - 勾选需要操作的数据库
   - 点击"批量操作"
   - 选择操作类型（解析/生成/更新）

3. **单项操作**
   - 点击数据库行的操作按钮
   - 支持：查看详情、重新解析、生成模型、清理缓存

### 3. 增量更新检测

1. **访问增量更新页面**
   ```
   http://localhost:8080/incremental
   ```

2. **启动检测**
   - 点击站点卡片的"检测"按钮
   - 系统将扫描文件变更

3. **执行同步**
   - 检测到变更后点击"同步"
   - 查看同步进度和历史记录

4. **配置自动检测**
   - 点击"配置"按钮
   - 设置自动检测间隔
   - 启用/禁用自动同步

### 4. 任务监控

1. **查看任务状态**
   - 任务启动后自动显示进度
   - 支持取消正在运行的任务

2. **查看任务日志**
   - 点击任务详情查看执行日志
   - 失败任务显示错误信息

---

## 故障排查

### 常见问题

#### 1. 数据库连接失败
```
错误: There was a problem with authentication
解决: 检查用户名密码是否正确，确认数据库已启动
```

#### 2. 端口被占用
```
错误: 端口已被占用
解决: 系统会自动尝试清理占用进程，或手动执行：
      kill -9 $(lsof -ti:8009)
```

#### 3. 解析失败
```
错误: 无效的PDMS数据格式
解决: 检查源数据文件是否完整，查看详细错误日志
```

### 日志位置
- WebUI日志: `web_ui.log`
- SurrealDB日志: `surreal.log`
- 任务日志: `logs/tasks/`

### 性能优化

1. **批量处理优化**
   ```toml
   sync_chunk_size = 100_0000  # 增加批量大小
   gen_model_batch_size = 100  # 调整模型生成批次
   ```

2. **并发控制**
   ```rust
   // 限制并发任务数
   const MAX_CONCURRENT_TASKS: usize = 5;
   ```

3. **缓存策略**
   - 启用 SQLite R-Tree 索引加速空间查询
   - 使用内存缓存热点数据

---

## 开发指南

### 添加新功能

1. **创建处理器**
   ```rust
   // src/web_ui/your_handler.rs
   pub async fn your_handler(
       State(state): State<AppState>,
   ) -> Result<Json<Value>, StatusCode> {
       // 实现逻辑
   }
   ```

2. **注册路由**
   ```rust
   // src/web_ui/mod.rs
   .route("/api/your-endpoint", get(your_handler))
   ```

3. **创建前端页面**
   ```javascript
   // src/web_ui/static/your_page.js
   class YourManager {
       async init() {
           // 初始化逻辑
       }
   }
   ```

### 测试
```bash
# 运行单元测试
cargo test --features "web_ui"

# 运行集成测试
cargo test --test integration_tests

# 性能测试
cargo bench
```

---

## 版本历史

### v1.0.0 (2024-01)
- 初始版本发布
- 基础数据库状态管理
- 数据库连接管理

### v1.1.0 (2024-02)
- 添加增量更新检测
- 批量操作支持
- 性能优化

### v1.2.0 (计划中)
- WebSocket 实时推送
- 任务调度系统
- 多租户支持

---

## 联系支持

- 项目仓库: https://github.com/your-repo/gen-model
- 问题反馈: https://github.com/your-repo/gen-model/issues
- 技术文档: https://docs.your-domain.com

---

## 许可证

MIT License - 详见 LICENSE 文件