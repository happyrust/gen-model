# LiteFS 异地协同数据库同步架构方案

## 需求背景

### 当前挑战
1. 在不同地区服务器上部署多个平台实例
2. 每个实例都需要访问和修改配置信息（deployment_sites, remote_sync_envs, remote_sync_sites）
3. 配置信息存储在 `deployment_sites.sqlite` 中
4. 需要实现配置信息的**实时双向同步**，避免手动配置

### 解决方案
使用 **LiteFS**（Distributed SQLite）实现 SQLite 数据库的自动同步和复制。

## LiteFS 简介

### 什么是 LiteFS？
LiteFS 是 Fly.io 开发的分布式 SQLite 文件系统，提供：
- **透明的 SQLite 复制**：自动将写操作同步到所有副本
- **读写分离**：一个主节点（Primary）+ 多个只读副本（Replica）
- **FUSE 文件系统**：通过 FUSE 挂载，应用程序无需修改代码
- **实时同步**：基于 SQLite WAL (Write-Ahead Logging) 的增量同步

### 架构模式

```
           ┌─────────────────────────────────────┐
           │      LiteFS Cluster                 │
           ├─────────────────────────────────────┤
           │                                     │
           │  ┌─────────────────────┐           │
           │  │  Primary Node       │           │
           │  │  (Beijing)          │           │
           │  │  - Write Enabled    │           │
           │  │  - deployment_sites │───────┐   │
           │  │    .sqlite          │       │   │
           │  └─────────────────────┘       │   │
           │            │                   │   │
           │            │ Sync              │   │
           │            ├───────────────────┼───┤
           │            │                   │   │
           │  ┌─────────▼────────┐  ┌──────▼───▼────┐
           │  │ Replica Node     │  │ Replica Node  │
           │  │ (Shanghai)       │  │ (Guangzhou)   │
           │  │ - Read Only      │  │ - Read Only   │
           │  │ - Auto Sync      │  │ - Auto Sync   │
           │  └──────────────────┘  └───────────────┘
           │                                     │
           └─────────────────────────────────────┘
```

## 实施方案

### 方案一：纯 LiteFS 方案（推荐）

#### 架构设计

```
每个服务器节点：
├── LiteFS FUSE 挂载点: /litefs
│   ├── deployment_sites.sqlite  (自动同步)
│   ├── deployment_sites.sqlite-shm
│   └── deployment_sites.sqlite-wal
│
├── Web UI 服务
│   └── 读写 /litefs/deployment_sites.sqlite
│
└── LiteFS Daemon
    ├── 监听 SQLite WAL 变化
    ├── 与其他节点通信（HTTP/gRPC）
    └── 同步数据变更
```

#### 优点
- ✅ 零代码修改，只需修改数据库路径配置
- ✅ 实时同步（秒级延迟）
- ✅ 自动处理冲突
- ✅ 支持故障转移（Primary 节点宕机后自动选举）

#### 缺点
- ⚠️ 需要部署 LiteFS daemon
- ⚠️ 写操作只能在 Primary 节点执行（其他节点只读）
- ⚠️ 需要配置节点间通信（HTTP/gRPC）

### 方案二：LiteFS + MQTT 混合方案

结合现有的 MQTT 同步机制：
- **LiteFS**：同步静态配置（deployment_sites, remote_sync_envs）
- **MQTT**：同步运行时数据和文件

#### 优点
- ✅ 充分利用现有 MQTT 基础设施
- ✅ 灵活的同步策略
- ✅ 可以同步大文件

#### 缺点
- ⚠️ 复杂度较高
- ⚠️ 需要维护两套同步机制

## 详细实施步骤

### Phase 1: LiteFS 环境准备（1 天）

#### 1.1 安装 LiteFS

在每个服务器节点上安装 LiteFS：

```bash
# 方法 1: 使用官方二进制
curl -fsSL https://fly.io/install.sh | sh
fly install litefs

# 方法 2: 使用 Docker（推荐）
docker pull flyio/litefs:latest

# 方法 3: 从源码编译
git clone https://github.com/superfly/litefs
cd litefs
go build -o litefs ./cmd/litefs
```

#### 1.2 创建 LiteFS 配置

创建 `/etc/litefs.yml`：

```yaml
# LiteFS 配置文件
fuse:
  # FUSE 挂载点
  dir: "/litefs"

data:
  # LiteFS 数据存储目录
  dir: "/var/lib/litefs"

# 主节点选举策略
lease:
  type: "consul"  # 或 "static" 用于固定主节点
  # Consul 配置（用于自动选举）
  consul:
    url: "http://consul:8500"
    key: "litefs/primary"

# 节点间通信
proxy:
  addr: ":20202"
  target: "localhost:8080"  # 应用实际监听端口
  db: "deployment_sites.sqlite"

# HTTP API（用于监控和管理）
http:
  addr: ":20203"

# 退出策略
exit:
  on-error: true
```

#### 1.3 配置静态主节点（简化方案）

如果不使用 Consul，可以使用静态配置：

```yaml
# /etc/litefs.yml
lease:
  type: "static"
  # 指定主节点
  advertise-url: "http://beijing-server:20202"
  # 当前节点是否为主节点
  is-primary: true  # 只在主节点设置为 true

# 副本节点配置
replica:
  # 主节点地址
  primary-url: "http://beijing-server:20202"
```

### Phase 2: 修改应用配置（0.5 天）

#### 2.1 修改数据库路径

在 `DbOption.toml` 中修改 SQLite 路径：

```toml
# 原配置
# deployment_sites_sqlite_path = "./deployment_sites.sqlite"

# 新配置（指向 LiteFS 挂载点）
deployment_sites_sqlite_path = "/litefs/deployment_sites.sqlite"
```

#### 2.2 修改数据库打开逻辑

在 `remote_sync_handlers.rs` 中，增强数据库连接：

```rust
fn open_sqlite() -> Result<rusqlite::Connection, Box<dyn std::error::Error>> {
    use config as cfg;

    let db_path = if std::path::Path::new("DbOption.toml").exists() {
        let builder = cfg::Config::builder()
            .add_source(cfg::File::with_name("DbOption"))
            .build()?;
        builder
            .get_string("deployment_sites_sqlite_path")
            .unwrap_or_else(|_| "/litefs/deployment_sites.sqlite".to_string())
    } else {
        "/litefs/deployment_sites.sqlite".to_string()
    };

    // 检查是否为 LiteFS 挂载点
    let is_litefs = db_path.starts_with("/litefs");

    let mut conn = rusqlite::Connection::open(&db_path)?;

    if is_litefs {
        // LiteFS 下推荐设置
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
    }

    // 创建表...
    Ok(conn)
}
```

#### 2.3 添加只读模式检测

由于副本节点是只读的，需要处理写操作失败：

```rust
pub async fn create_env(
    Json(req): Json<CreateEnvRequest>
) -> Result<Json<serde_json::Value>, StatusCode> {
    match do_create_env(req) {
        Ok(result) => Ok(Json(result)),
        Err(e) if e.to_string().contains("readonly") => {
            // 如果是只读错误，返回友好提示
            Ok(Json(json!({
                "status": "error",
                "message": "当前节点为只读副本，请在主节点执行写操作",
                "is_replica": true,
                "primary_url": get_primary_url() // 返回主节点地址
            })))
        }
        Err(e) => {
            eprintln!("创建环境失败: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// 从 LiteFS API 获取主节点地址
fn get_primary_url() -> Option<String> {
    // LiteFS HTTP API: http://localhost:20203/status
    // 返回包含 primary 信息的 JSON
    // TODO: 实现从 LiteFS API 获取主节点信息
    None
}
```

### Phase 3: Docker 部署方案（1 天）

#### 3.1 创建 Dockerfile

```dockerfile
FROM rust:1.75 as builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin web_ui

FROM ubuntu:22.04

# 安装 LiteFS 和依赖
RUN apt-get update && apt-get install -y \
    ca-certificates \
    fuse3 \
    sqlite3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# 安装 LiteFS
RUN curl -L https://github.com/superfly/litefs/releases/download/v0.5.11/litefs-v0.5.11-linux-amd64.tar.gz | tar xz -C /usr/local/bin

# 复制应用
COPY --from=builder /app/target/release/web_ui /usr/local/bin/
COPY DbOption.toml /app/
COPY litefs.yml /etc/litefs.yml

# 创建挂载点
RUN mkdir -p /litefs /var/lib/litefs

# 启动脚本
COPY docker-entrypoint.sh /
RUN chmod +x /docker-entrypoint.sh

WORKDIR /app
EXPOSE 8080 20202 20203

ENTRYPOINT ["/docker-entrypoint.sh"]
```

#### 3.2 创建启动脚本

`docker-entrypoint.sh`:

```bash
#!/bin/bash
set -e

# 启动 LiteFS（后台运行）
litefs mount &
LITEFS_PID=$!

# 等待 LiteFS 挂载完成
for i in {1..30}; do
    if [ -d "/litefs" ]; then
        echo "LiteFS mounted successfully"
        break
    fi
    echo "Waiting for LiteFS to mount... ($i/30)"
    sleep 1
done

# 如果是主节点，初始化数据库
if [ "$IS_PRIMARY" = "true" ]; then
    echo "Initializing database on primary node..."
    # 这里可以运行初始化脚本
fi

# 启动应用
echo "Starting web_ui service..."
exec /usr/local/bin/web_ui
```

#### 3.3 Docker Compose 配置

`docker-compose.yml`:

```yaml
version: '3.8'

services:
  # Consul（用于主节点选举）
  consul:
    image: consul:latest
    ports:
      - "8500:8500"
    command: agent -server -bootstrap-expect=1 -ui -client=0.0.0.0

  # 北京节点（主节点）
  web-ui-beijing:
    build: .
    ports:
      - "8080:8080"
      - "20202:20202"
      - "20203:20203"
    environment:
      - IS_PRIMARY=true
      - LITEFS_CONSUL_URL=http://consul:8500
      - NODE_NAME=beijing
    volumes:
      - ./litefs-beijing.yml:/etc/litefs.yml
      - litefs-beijing-data:/var/lib/litefs
    cap_add:
      - SYS_ADMIN  # 需要 FUSE 权限
    devices:
      - /dev/fuse
    depends_on:
      - consul

  # 上海节点（副本）
  web-ui-shanghai:
    build: .
    ports:
      - "8081:8080"
      - "20212:20202"
      - "20213:20203"
    environment:
      - IS_PRIMARY=false
      - LITEFS_CONSUL_URL=http://consul:8500
      - NODE_NAME=shanghai
      - PRIMARY_URL=http://web-ui-beijing:20202
    volumes:
      - ./litefs-shanghai.yml:/etc/litefs.yml
      - litefs-shanghai-data:/var/lib/litefs
    cap_add:
      - SYS_ADMIN
    devices:
      - /dev/fuse
    depends_on:
      - consul
      - web-ui-beijing

  # 广州节点（副本）
  web-ui-guangzhou:
    build: .
    ports:
      - "8082:8080"
      - "20222:20202"
      - "20223:20203"
    environment:
      - IS_PRIMARY=false
      - LITEFS_CONSUL_URL=http://consul:8500
      - NODE_NAME=guangzhou
      - PRIMARY_URL=http://web-ui-beijing:20202
    volumes:
      - ./litefs-guangzhou.yml:/etc/litefs.yml
      - litefs-guangzhou-data:/var/lib/litefs
    cap_add:
      - SYS_ADMIN
    devices:
      - /dev/fuse
    depends_on:
      - consul
      - web-ui-beijing

volumes:
  litefs-beijing-data:
  litefs-shanghai-data:
  litefs-guangzhou-data:
```

### Phase 4: 前端智能路由（1 天）

#### 4.1 添加节点状态检测 API

在 `src/web_ui/mod.rs` 中添加：

```rust
// 获取当前节点状态
pub async fn node_status() -> Json<serde_json::Value> {
    let is_primary = check_if_primary();
    let primary_url = get_primary_url();

    Json(json!({
        "is_primary": is_primary,
        "primary_url": primary_url,
        "node_name": env::var("NODE_NAME").ok(),
        "litefs_status": get_litefs_status()
    }))
}

fn check_if_primary() -> bool {
    // 尝试写入测试
    match open_sqlite() {
        Ok(conn) => {
            match conn.execute("CREATE TABLE IF NOT EXISTS _litefs_test (id INTEGER)", []) {
                Ok(_) => true,
                Err(e) if e.to_string().contains("readonly") => false,
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}

fn get_litefs_status() -> serde_json::Value {
    // 调用 LiteFS HTTP API
    match reqwest::blocking::get("http://localhost:20203/status") {
        Ok(resp) => resp.json().unwrap_or(json!({})),
        Err(_) => json!({"error": "Cannot connect to LiteFS"})
    }
}
```

#### 4.2 前端自动重定向

在 `collaboration-adapter.ts` 中添加：

```typescript
// 检测节点状态
async function checkNodeStatus(): Promise<NodeStatus> {
  const response = await fetch(buildApiUrl("/api/node-status"))
  return handleResponse<NodeStatus>(response)
}

// 自动重定向到主节点
async function ensurePrimaryNode(): Promise<void> {
  const status = await checkNodeStatus()
  if (!status.is_primary && status.primary_url) {
    // 重定向到主节点
    const primaryOrigin = new URL(status.primary_url).origin
    if (window.location.origin !== primaryOrigin) {
      console.log(`Redirecting to primary node: ${primaryOrigin}`)
      window.location.href = primaryOrigin + window.location.pathname
    }
  }
}

// 修改写操作，添加重定向逻辑
export async function createRemoteSyncEnv(payload: RemoteSyncEnvCreatePayload): Promise<RemoteSyncEnv> {
  try {
    const response = await fetch(buildApiUrl("/api/remote-sync/envs"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    })
    const result = await handleResponse<RemoteSyncEnv>(response)
    return result
  } catch (error: any) {
    // 如果是只读错误，尝试重定向到主节点
    if (error.message?.includes("readonly") || error.is_replica) {
      await ensurePrimaryNode()
      throw new Error("正在重定向到主节点...")
    }
    throw error
  }
}
```

### Phase 5: 监控和运维（1 天）

#### 5.1 添加健康检查

```rust
// 健康检查端点
pub async fn health_check() -> Json<serde_json::Value> {
    let db_status = match open_sqlite() {
        Ok(_) => "healthy",
        Err(_) => "unhealthy"
    };

    let litefs_status = get_litefs_status();

    Json(json!({
        "status": if db_status == "healthy" { "ok" } else { "error" },
        "database": db_status,
        "litefs": litefs_status,
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}
```

#### 5.2 监控仪表板

在前端添加同步状态监控：

```typescript
// 定期检查同步状态
function useLiteFSStatus() {
  const [status, setStatus] = useState<LiteFSStatus | null>(null)

  useEffect(() => {
    const fetchStatus = async () => {
      try {
        const response = await fetch("/api/node-status")
        const data = await response.json()
        setStatus(data)
      } catch (error) {
        console.error("Failed to fetch LiteFS status", error)
      }
    }

    fetchStatus()
    const interval = setInterval(fetchStatus, 5000) // 每 5 秒刷新

    return () => clearInterval(interval)
  }, [])

  return status
}
```

## 方案对比

### LiteFS vs 其他方案

| 方案 | 优点 | 缺点 | 适用场景 |
|------|------|------|---------|
| **LiteFS** | 零代码修改，实时同步，自动故障转移 | 需要部署 daemon，写操作限制 | 配置同步（推荐）|
| **Litestream** | 简单，S3 备份 | 单向复制，恢复慢 | 备份恢复 |
| **自定义 MQTT** | 灵活，已有基础设施 | 需要大量开发，复杂 | 文件同步 |
| **rqlite** | 分布式 SQL，强一致性 | 需要替换 SQLite | 需要 SQL 接口 |
| **FoundationDB** | 强一致性，高性能 | 复杂，重量级 | 大规模部署 |

## 混合架构（终极方案）

结合 LiteFS + MQTT，实现完整的异地协同：

```
┌─────────────────────────────────────────────────────┐
│              Multi-Region Deployment                │
├─────────────────────────────────────────────────────┤
│                                                     │
│  [配置同步层 - LiteFS]                              │
│    ├─ deployment_sites.sqlite                      │
│    ├─ remote_sync_envs (环境配置)                  │
│    └─ remote_sync_sites (站点配置)                 │
│         ↓ ↑                                         │
│    实时双向同步（秒级）                             │
│                                                     │
│  ────────────────────────────────────────────      │
│                                                     │
│  [运行时数据层 - MQTT]                              │
│    ├─ 文件传输（sync_files/）                      │
│    ├─ 实时状态更新                                  │
│    └─ 任务协调                                      │
│         ↓ ↑                                         │
│    发布/订阅模式                                    │
│                                                     │
└─────────────────────────────────────────────────────┘
```

### 数据分层策略

| 数据类型 | 同步方式 | 延迟 | 一致性 |
|---------|---------|------|--------|
| 配置信息（envs, sites） | LiteFS | <1s | 强一致性 |
| 文件数据（tar.gz, db） | MQTT + HTTP | <5s | 最终一致性 |
| 实时状态（sync status） | MQTT | <100ms | 最终一致性 |

## 实施时间表

| 阶段 | 任务 | 时间 |
|-----|------|------|
| Phase 1 | LiteFS 环境准备 | 1 天 |
| Phase 2 | 修改应用配置 | 0.5 天 |
| Phase 3 | Docker 部署方案 | 1 天 |
| Phase 4 | 前端智能路由 | 1 天 |
| Phase 5 | 监控和运维 | 1 天 |
| **总计** | | **4.5 天** |

## 快速启动指南

### 本地测试（Docker Compose）

```bash
# 1. 构建镜像
docker-compose build

# 2. 启动所有节点
docker-compose up -d

# 3. 检查状态
docker-compose ps
docker-compose logs -f web-ui-beijing

# 4. 访问不同节点
# 北京（主节点）: http://localhost:8080
# 上海（副本）: http://localhost:8081
# 广州（副本）: http://localhost:8082

# 5. 测试同步
# 在主节点创建环境，观察副本节点自动同步
```

### 生产部署（多服务器）

```bash
# 在每个服务器上：

# 1. 安装 LiteFS
curl -L https://github.com/superfly/litefs/releases/download/v0.5.11/litefs-v0.5.11-linux-amd64.tar.gz | tar xz -C /usr/local/bin

# 2. 配置 LiteFS
sudo cp litefs-{node}.yml /etc/litefs.yml

# 3. 启动 LiteFS（systemd）
sudo systemctl enable litefs
sudo systemctl start litefs

# 4. 启动应用
./target/release/web_ui
```

## 故障处理

### 常见问题

1. **主节点宕机**
   - LiteFS 自动选举新主节点（使用 Consul）
   - 应用需要重启以连接新主节点

2. **网络分区**
   - 副本节点继续提供只读服务
   - 网络恢复后自动同步

3. **数据冲突**
   - LiteFS 使用 MVCC 处理并发
   - 冲突较少（配置变更频率低）

## 总结

使用 LiteFS 实现 SQLite 数据库的异地同步，可以：

✅ **零配置部署**：在任何服务器上部署，自动同步配置
✅ **实时同步**：秒级延迟，配置变更立即生效
✅ **高可用性**：主节点宕机自动切换
✅ **零代码修改**：只需修改数据库路径配置
✅ **混合架构**：LiteFS（配置）+ MQTT（文件）= 完整方案

推荐的最终架构：
- **LiteFS**：同步 deployment_sites.sqlite（配置信息）
- **MQTT**：同步运行时数据和大文件
- **内置文件服务器**：提供 HTTP 文件下载

实施后，用户体验：
1. 在任意服务器创建环境配置
2. 配置自动同步到所有服务器
3. 在任意服务器查看完整配置
4. 写操作自动路由到主节点