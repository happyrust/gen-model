# 内置文件服务器集成方案

## 需求分析

### 当前状态
- 创建远程同步环境时，需要手动输入 `file_server_host`（可选）
- 文件服务器用于异地协同时，各站点通过 HTTP 共享和下载文件
- 目前只有 HTTP 可达性测试，没有实际的文件上传/下载功能
- 文件服务器 URL 会被写入 `DbOption.toml` 配置文件

### 用户需求
希望在 API 服务中内置文件服务器功能，这样：
1. 不需要单独部署文件服务器
2. 不需要手动输入文件服务器 URL
3. 创建环境时自动使用本地服务器地址

## 技术方案

### 1. 文件存储架构

#### 存储目录结构
```
sync_files/           # 文件存储根目录
├── uploads/          # 上传文件存储
│   ├── {env_id}/     # 按环境分组
│   │   └── {site_id}/  # 按站点分组
│   │       └── {timestamp}_{filename}  # 文件
└── metadata/         # 文件元数据（可选）
```

#### 存储位置选择
- **方案 A**：项目根目录 `./sync_files/`（推荐）
  - 优点：简单直接，易于管理
  - 缺点：与代码混在一起

- **方案 B**：用户目录 `~/.gen_model/sync_files/`
  - 优点：符合规范，不影响项目
  - 缺点：需要处理跨用户权限问题

**推荐使用方案 A**，创建 `.gitignore` 排除 `sync_files/` 目录。

### 2. API 端点设计

#### 2.1 文件上传
```
POST /api/sync-files/upload
Content-Type: multipart/form-data

Request:
- env_id: string (环境 ID)
- site_id: string (站点 ID)
- file: binary (文件内容)

Response:
{
  "status": "success",
  "file_id": "file_abc123",
  "file_name": "example.tar.gz",
  "file_size": 1024000,
  "file_path": "/api/sync-files/download/file_abc123",
  "uploaded_at": "2025-09-28T10:00:00Z"
}
```

#### 2.2 文件下载
```
GET /api/sync-files/download/{file_id}

Response:
- Content-Type: application/octet-stream
- Content-Disposition: attachment; filename="example.tar.gz"
- Binary file content
```

#### 2.3 文件列表
```
GET /api/sync-files/list?env_id={env_id}&site_id={site_id}

Response:
{
  "status": "success",
  "files": [
    {
      "file_id": "file_abc123",
      "file_name": "example.tar.gz",
      "file_size": 1024000,
      "env_id": "env_001",
      "site_id": "site_001",
      "uploaded_at": "2025-09-28T10:00:00Z",
      "download_url": "/api/sync-files/download/file_abc123"
    }
  ],
  "total": 1
}
```

#### 2.4 文件删除（可选）
```
DELETE /api/sync-files/{file_id}

Response:
{
  "status": "success",
  "message": "文件已删除"
}
```

### 3. 数据库表设计

在 SQLite 中新增 `sync_files` 表：

```sql
CREATE TABLE IF NOT EXISTS sync_files (
    id TEXT PRIMARY KEY,           -- 文件 ID (file_{uuid})
    env_id TEXT NOT NULL,          -- 环境 ID
    site_id TEXT,                  -- 站点 ID（可选）
    file_name TEXT NOT NULL,       -- 原始文件名
    file_size INTEGER NOT NULL,    -- 文件大小（字节）
    file_path TEXT NOT NULL,       -- 文件存储路径
    mime_type TEXT,                -- MIME 类型
    uploaded_at TEXT NOT NULL,     -- 上传时间
    created_at TEXT NOT NULL,      -- 创建时间
    FOREIGN KEY (env_id) REFERENCES remote_sync_envs(id) ON DELETE CASCADE
);

CREATE INDEX idx_sync_files_env ON sync_files(env_id);
CREATE INDEX idx_sync_files_site ON sync_files(site_id);
```

### 4. 自动配置 file_server_host

#### 4.1 获取服务器地址
在创建环境时，自动填充 `file_server_host`：

```rust
// 从请求头中获取 Host
fn get_server_base_url(headers: &HeaderMap) -> Option<String> {
    let host = headers.get("host")?.to_str().ok()?;
    let scheme = if headers.get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|s| s == "https")
        .unwrap_or(false)
    {
        "https"
    } else {
        "http"
    };
    Some(format!("{}://{}/api/sync-files", scheme, host))
}
```

#### 4.2 修改创建环境逻辑

```rust
pub async fn create_env(
    TypedHeader(headers): TypedHeader<HeaderMap>,
    Json(mut req): Json<CreateEnvRequest>
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 如果未提供 file_server_host，自动填充
    if req.file_server_host.is_none() {
        req.file_server_host = get_server_base_url(&headers);
    }

    // ... 后续创建逻辑
}
```

### 5. 前端 UI 修改

#### 5.1 创建对话框修改
在 `create-group-dialog.tsx` 中：

```typescript
// 移除文件服务器输入框，或改为只读自动填充
<div className="space-y-2">
  <Label htmlFor="file-server">文件服务器地址（自动）</Label>
  <Input
    id="file-server"
    value={`${window.location.origin}/api/sync-files`}
    readOnly
    className="bg-muted"
  />
  <p className="text-xs text-muted-foreground">
    使用内置文件服务器，无需配置
  </p>
</div>
```

#### 5.2 适配器修改
在 `collaboration-adapter.ts` 中：

```typescript
export function groupToEnvPayload(group: Partial<CollaborationGroup>): RemoteSyncEnvCreatePayload {
  return {
    name: group.name || "",
    mqtt_host: group.shared_config?.mqtt_broker || "",
    mqtt_port: group.shared_config?.mqtt_port || 1883,
    mqtt_user: group.shared_config?.mqtt_username,
    mqtt_password: group.shared_config?.mqtt_password,
    // file_server_host 不传，让后端自动填充
    // file_server_host: undefined,
    location: group.location,
    reconnect_initial_ms: 1000,
    reconnect_max_ms: 60000,
  }
}
```

### 6. 实现步骤

#### Phase 1: 后端基础设施（1-2 天）
1. 创建 `src/web_server/sync_file_handlers.rs` 模块
2. 实现文件存储目录初始化
3. 创建 `sync_files` 数据库表
4. 实现文件上传 API
5. 实现文件下载 API
6. 实现文件列表 API

#### Phase 2: 自动配置（0.5 天）
1. 实现 `get_server_base_url()` 函数
2. 修改 `create_env()` 自动填充 file_server_host
3. 修改 `update_env()` 保持兼容性（允许手动指定）

#### Phase 3: 前端集成（0.5 天）
1. 修改创建对话框，显示自动配置的文件服务器地址
2. 更新适配器，不传递 file_server_host（让后端自动填充）
3. 测试创建环境流程

#### Phase 4: 测试和优化（1 天）
1. 测试文件上传/下载功能
2. 测试多环境、多站点场景
3. 添加文件大小限制和类型验证
4. 添加文件清理机制（可选）

### 7. 安全考虑

#### 7.1 文件大小限制
```rust
const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100MB
```

#### 7.2 文件类型验证
只允许特定类型：
- `.tar.gz`
- `.zip`
- `.db`
- `.sqlite`

#### 7.3 路径遍历防护
验证文件名，防止 `../` 等路径遍历攻击：
```rust
fn sanitize_filename(filename: &str) -> String {
    filename
        .replace("../", "")
        .replace("..\\", "")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect()
}
```

#### 7.4 访问控制
- 只允许属于同一环境的站点访问文件
- 考虑添加简单的 token 验证机制

### 8. 配置选项（可选）

在 `DbOption.toml` 中添加配置：

```toml
[file_server]
enabled = true                    # 是否启用内置文件服务器
storage_path = "./sync_files"     # 存储路径
max_file_size = 104857600         # 最大文件大小（字节）
allowed_extensions = [".tar.gz", ".zip", ".db", ".sqlite"]
auto_cleanup_days = 30            # 自动清理超过 N 天的文件（0 = 禁用）
```

### 9. 性能优化

#### 9.1 流式传输
对于大文件，使用流式传输而非一次性加载到内存：

```rust
use tokio::fs::File;
use tokio_util::io::ReaderStream;
use axum::body::StreamBody;

pub async fn download_file(Path(file_id): Path<String>) -> Result<Response, StatusCode> {
    let file_path = get_file_path(&file_id)?;
    let file = File::open(&file_path).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let stream = ReaderStream::new(file);
    let body = StreamBody::new(stream);

    Ok(Response::builder()
        .header("content-type", "application/octet-stream")
        .header("content-disposition", format!("attachment; filename=\"{}\"", filename))
        .body(body)
        .unwrap())
}
```

#### 9.2 缓存策略
为下载的文件添加 HTTP 缓存头：
```rust
.header("cache-control", "public, max-age=86400")
.header("etag", calculate_etag(&file_path))
```

### 10. 示例代码结构

```rust
// src/web_server/sync_file_handlers.rs

use axum::{
    extract::{Path, Query, State, Multipart},
    http::{StatusCode, HeaderMap},
    Json, response::Response,
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct UploadQuery {
    pub env_id: String,
    pub site_id: Option<String>,
}

#[derive(Serialize)]
pub struct FileInfo {
    pub file_id: String,
    pub file_name: String,
    pub file_size: u64,
    pub env_id: String,
    pub site_id: Option<String>,
    pub uploaded_at: String,
    pub download_url: String,
}

pub async fn upload_file(
    Query(query): Query<UploadQuery>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // TODO: 实现文件上传逻辑
}

pub async fn download_file(
    Path(file_id): Path<String>,
) -> Result<Response, StatusCode> {
    // TODO: 实现文件下载逻辑
}

pub async fn list_files(
    Query(query): Query<ListFilesQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // TODO: 实现文件列表逻辑
}
```

### 11. 路由注册

在 `src/web_server/mod.rs` 中添加路由：

```rust
use sync_file_handlers::{upload_file, download_file, list_files};

let app = Router::new()
    // ... 现有路由
    .route("/api/sync-files/upload", post(upload_file))
    .route("/api/sync-files/download/:file_id", get(download_file))
    .route("/api/sync-files/list", get(list_files))
    .route("/api/sync-files/:file_id", delete(delete_file))
    // ... 其他路由
```

## 总结

本方案通过在现有 web_server 服务中内置文件服务器功能，实现了：
1. **零配置**：创建环境时自动配置文件服务器地址
2. **集成化**：无需单独部署文件服务器
3. **简化部署**：一个服务包含所有功能
4. **安全性**：文件隔离、大小限制、类型验证

预计实施时间：**2-3 天**

## 下一步行动

1. 确认方案是否符合需求
2. 开始实施 Phase 1：后端基础设施
3. 逐步完成各个阶段
4. 测试和优化

## 附录：兼容性

为保持向后兼容，允许用户：
1. 手动指定外部文件服务器 URL（优先级更高）
2. 禁用内置文件服务器
3. 混合使用内置和外部文件服务器