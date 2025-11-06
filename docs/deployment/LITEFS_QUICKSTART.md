# LiteFS 异地协同快速入门

## 5 分钟理解方案

### 问题
你在北京、上海、广州各部署了一个平台实例，希望：
- 在北京创建的环境配置，上海和广州能立即看到
- 在上海创建的站点配置，北京和广州能立即看到
- 不需要手动配置，自动同步

### 解决方案
使用 **LiteFS** 同步 SQLite 数据库

### 工作原理

```
北京服务器（主节点）              上海服务器（副本）           广州服务器（副本）
    │                                  │                          │
    │  创建环境配置                    │                          │
    │  ↓                                │                          │
    │  写入 SQLite                      │                          │
    │  ↓                                │                          │
    │  LiteFS 检测到变更                │                          │
    │  ↓                                │                          │
    │  ─────── 同步 ──────────────────→ │                          │
    │                                   │  接收同步                │
    │                                   │  ↓                       │
    │                                   │  更新本地 SQLite         │
    │                                   │  ↓                       │
    │  ─────── 同步 ───────────────────────────────────────────→ │
    │                                   │                          │  接收同步
    │                                   │                          │  ↓
    │                                   │                          │  更新本地 SQLite
    │                                   │                          │
    ✓ 配置已同步到所有节点              ✓                          ✓
```

## 最简单的实施方案（Docker）

### 前提条件
- 安装了 Docker 和 Docker Compose
- 3 台服务器（或本地测试）

### 步骤 1：准备配置文件

创建 3 个 LiteFS 配置文件：

**litefs-beijing.yml**（主节点）:
```yaml
fuse:
  dir: "/litefs"
data:
  dir: "/var/lib/litefs"
lease:
  type: "static"
  is-primary: true
  advertise-url: "http://beijing-server:20202"
proxy:
  addr: ":20202"
  target: "localhost:8080"
  db: "deployment_sites.sqlite"
http:
  addr: ":20203"
```

**litefs-shanghai.yml**（副本）:
```yaml
fuse:
  dir: "/litefs"
data:
  dir: "/var/lib/litefs"
lease:
  type: "static"
  is-primary: false
replica:
  primary-url: "http://beijing-server:20202"
proxy:
  addr: ":20202"
  target: "localhost:8080"
  db: "deployment_sites.sqlite"
http:
  addr: ":20203"
```

**litefs-guangzhou.yml**（副本）:
```yaml
fuse:
  dir: "/litefs"
data:
  dir: "/var/lib/litefs"
lease:
  type: "static"
  is-primary: false
replica:
  primary-url: "http://beijing-server:20202"
proxy:
  addr: ":20202"
  target: "localhost:8080"
  db: "deployment_sites.sqlite"
http:
  addr: ":20203"
```

### 步骤 2：修改 DbOption.toml

```toml
# 修改数据库路径指向 LiteFS 挂载点
deployment_sites_sqlite_path = "/litefs/deployment_sites.sqlite"
```

### 步骤 3：创建 Dockerfile

```dockerfile
FROM rust:1.75 as builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin web_server

FROM ubuntu:22.04

# 安装依赖
RUN apt-get update && apt-get install -y \
    ca-certificates \
    fuse3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# 安装 LiteFS
ADD https://github.com/superfly/litefs/releases/download/v0.5.11/litefs-v0.5.11-linux-amd64.tar.gz /tmp/litefs.tar.gz
RUN tar -xzf /tmp/litefs.tar.gz -C /usr/local/bin && rm /tmp/litefs.tar.gz

# 复制应用
COPY --from=builder /app/target/release/web_server /usr/local/bin/
COPY --from=builder /app/DbOption.toml /app/
COPY --from=builder /app/src/web_server /app/src/web_server

# 创建目录
RUN mkdir -p /litefs /var/lib/litefs /app

WORKDIR /app
EXPOSE 8080 20202 20203

# 启动脚本
COPY <<'EOF' /entrypoint.sh
#!/bin/bash
set -e

# 复制配置文件
cp /config/litefs.yml /etc/litefs.yml

# 启动 LiteFS
litefs mount &

# 等待挂载
sleep 3

# 启动应用
exec /usr/local/bin/web_server
EOF

RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
```

### 步骤 4：启动服务

**北京服务器**:
```bash
docker run -d \
  --name web-ui-beijing \
  --cap-add SYS_ADMIN \
  --device /dev/fuse \
  -p 8080:8080 \
  -p 20202:20202 \
  -v $(pwd)/litefs-beijing.yml:/config/litefs.yml:ro \
  -v litefs-beijing:/var/lib/litefs \
  web-ui:latest
```

**上海服务器**:
```bash
docker run -d \
  --name web-ui-shanghai \
  --cap-add SYS_ADMIN \
  --device /dev/fuse \
  -p 8080:8080 \
  -p 20202:20202 \
  -v $(pwd)/litefs-shanghai.yml:/config/litefs.yml:ro \
  -v litefs-shanghai:/var/lib/litefs \
  web-ui:latest
```

**广州服务器**:
```bash
docker run -d \
  --name web-ui-guangzhou \
  --cap-add SYS_ADMIN \
  --device /dev/fuse \
  -p 8080:8080 \
  -p 20202:20202 \
  -v $(pwd)/litefs-guangzhou.yml:/config/litefs.yml:ro \
  -v litefs-guangzhou:/var/lib/litefs \
  web-ui:latest
```

### 步骤 5：验证同步

```bash
# 在北京服务器
curl http://beijing-server:8080/api/remote-sync/envs

# 在上海服务器（应该看到相同的数据）
curl http://shanghai-server:8080/api/remote-sync/envs

# 在广州服务器（应该看到相同的数据）
curl http://guangzhou-server:8080/api/remote-sync/envs
```

## 本地测试（Docker Compose）

### docker-compose.yml

```yaml
version: '3.8'

services:
  beijing:
    build: .
    ports:
      - "8080:8080"
    volumes:
      - ./litefs-beijing.yml:/config/litefs.yml:ro
    cap_add:
      - SYS_ADMIN
    devices:
      - /dev/fuse
    networks:
      litefs-net:
        aliases:
          - beijing-server

  shanghai:
    build: .
    ports:
      - "8081:8080"
    volumes:
      - ./litefs-shanghai.yml:/config/litefs.yml:ro
    cap_add:
      - SYS_ADMIN
    devices:
      - /dev/fuse
    networks:
      litefs-net:
        aliases:
          - shanghai-server
    depends_on:
      - beijing

  guangzhou:
    build: .
    ports:
      - "8082:8080"
    volumes:
      - ./litefs-guangzhou.yml:/config/litefs.yml:ro
    cap_add:
      - SYS_ADMIN
    devices:
      - /dev/fuse
    networks:
      litefs-net:
        aliases:
          - guangzhou-server
    depends_on:
      - beijing

networks:
  litefs-net:
    driver: bridge
```

### 启动测试

```bash
# 启动所有服务
docker-compose up -d

# 查看日志
docker-compose logs -f

# 测试主节点（北京）
curl http://localhost:8080/api/remote-sync/envs

# 测试副本（上海）
curl http://localhost:8081/api/remote-sync/envs

# 测试副本（广州）
curl http://localhost:8082/api/remote-sync/envs
```

## 常见问题

### Q1: 副本节点能写入吗？
**A**: 不能。只有主节点（北京）可以写入，副本只能读取。写操作会自动重定向到主节点。

### Q2: 主节点宕机怎么办？
**A**: 需要手动将某个副本提升为主节点，或使用 Consul 自动选举。

### Q3: 同步延迟多久？
**A**: 通常 < 1 秒。LiteFS 使用 SQLite WAL 模式，增量同步非常快。

### Q4: 数据库文件多大合适？
**A**: LiteFS 适合小型数据库（< 10GB）。配置数据通常很小（< 100MB）。

### Q5: 支持多主吗？
**A**: 不支持。LiteFS 是单主多副本架构。

## 进阶：添加智能路由

### 后端修改

在 `src/web_server/mod.rs` 添加中间件：

```rust
use axum::{
    middleware::{self, Next},
    http::{Request, StatusCode},
    response::{Response, IntoResponse},
};

async fn redirect_writes_to_primary<B>(
    req: Request<B>,
    next: Next<B>,
) -> Result<Response, StatusCode> {
    // 如果是写操作（POST, PUT, DELETE）且当前是副本节点
    if matches!(req.method(), &Method::POST | &Method::PUT | &Method::DELETE) {
        if !is_primary_node() {
            // 返回重定向响应
            let primary_url = get_primary_url().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
            let redirect_url = format!("{}{}", primary_url, req.uri().path());

            return Ok((
                StatusCode::TEMPORARY_REDIRECT,
                [("Location", redirect_url)]
            ).into_response());
        }
    }

    Ok(next.run(req).await)
}

// 应用中间件
let app = Router::new()
    .route("/api/remote-sync/envs", post(create_env))
    .layer(middleware::from_fn(redirect_writes_to_primary))
    // ... 其他路由
```

### 前端修改

在 `collaboration-adapter.ts` 中：

```typescript
// 自动跟随重定向
export async function createRemoteSyncEnv(
  payload: RemoteSyncEnvCreatePayload
): Promise<RemoteSyncEnv> {
  const response = await fetch(buildApiUrl("/api/remote-sync/envs"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
    redirect: "follow", // 自动跟随 307 重定向
  })
  return handleResponse<RemoteSyncEnv>(response)
}
```

## 监控和运维

### 检查同步状态

```bash
# 查看 LiteFS 状态
curl http://localhost:20203/status

# 返回示例
{
  "is_primary": true,
  "primary": "beijing-server:20202",
  "position": "000001a2b3c4d5e6/123456",
  "replicas": [
    {
      "name": "shanghai-server:20202",
      "position": "000001a2b3c4d5e6/123456"
    },
    {
      "name": "guangzhou-server:20202",
      "position": "000001a2b3c4d5e6/123456"
    }
  ]
}
```

### 健康检查

```bash
# 添加健康检查端点
curl http://localhost:8080/api/health

# 返回
{
  "status": "ok",
  "database": "healthy",
  "litefs": {
    "is_primary": true,
    "sync_lag": 0
  }
}
```

## 生产环境建议

### 1. 使用 Consul 自动选举

```yaml
# litefs.yml
lease:
  type: "consul"
  consul:
    url: "http://consul:8500"
    key: "litefs/primary"
    ttl: "10s"
    lock-delay: "5s"
```

### 2. 配置备份

```bash
# 定期备份主节点数据库
0 2 * * * rsync -avz /var/lib/litefs/deployment_sites.sqlite /backup/$(date +\%Y\%m\%d).sqlite
```

### 3. 监控告警

```bash
# Prometheus metrics
curl http://localhost:20203/metrics

# 配置告警规则
- alert: LiteFSSyncLag
  expr: litefs_sync_lag_seconds > 10
  for: 5m
  labels:
    severity: warning
  annotations:
    summary: "LiteFS sync lag detected"
```

## 总结

使用 LiteFS，你可以：

✅ **3 台服务器，3 条命令**，完成部署
✅ **零配置同步**，创建环境自动分发
✅ **秒级同步**，配置变更立即生效
✅ **高可用性**，主节点宕机可切换

下一步：
1. 本地测试 Docker Compose 方案
2. 验证同步功能
3. 部署到生产环境
4. 配置监控和告警