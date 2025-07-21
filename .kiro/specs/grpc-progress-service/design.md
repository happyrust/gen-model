# Design Document

## Overview

本设计文档描述了一个GRPC微服务的实现方案，该服务将为AIOS数据库解析系统提供Web界面支持。服务将集成到现有的Rust项目中，提供实时进度监控、MDB管理和任务控制功能。

## Architecture

### 系统架构图

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│   Web Frontend  │    │  GRPC Gateway   │    │  Rust Backend   │
│   (React/Vue)   │◄──►│   (tonic-web)   │◄──►│   (tonic)       │
└─────────────────┘    └─────────────────┘    └─────────────────┘
                                │                       │
                                │                       │
                                ▼                       ▼
                       ┌─────────────────┐    ┌─────────────────┐
                       │   HTTP Proxy    │    │   SurrealDB     │
                       │   (Envoy/Nginx) │    │   Database      │
                       └─────────────────┘    └─────────────────┘
```

### 服务层次结构

1. **Presentation Layer**: Web前端界面
2. **API Gateway Layer**: GRPC-Web网关和HTTP代理
3. **Service Layer**: GRPC服务实现
4. **Business Logic Layer**: 解析任务管理和进度跟踪
5. **Data Access Layer**: 数据库访问和MDB文件管理

## Components and Interfaces

### 1. GRPC服务定义

#### Proto文件结构
```protobuf
// progress_service.proto
syntax = "proto3";

package progress_service;

// 进度服务
service ProgressService {
  // 获取解析进度流
  rpc GetProgressStream(ProgressRequest) returns (stream ProgressResponse);
  
  // 获取MDB列表
  rpc GetMdbList(MdbListRequest) returns (MdbListResponse);
  
  // 获取MDB详情
  rpc GetMdbDetails(MdbDetailsRequest) returns (MdbDetailsResponse);
  
  // 启动解析任务
  rpc StartParseTask(StartTaskRequest) returns (TaskResponse);
  
  // 停止解析任务
  rpc StopParseTask(StopTaskRequest) returns (TaskResponse);
  
  // 获取任务状态
  rpc GetTaskStatus(TaskStatusRequest) returns (TaskStatusResponse);
  
  // 健康检查
  rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
}
```

### 2. 核心组件

#### ProgressService实现
```rust
pub struct ProgressServiceImpl {
    progress_manager: Arc<ProgressManager>,
    mdb_manager: Arc<MdbManager>,
    task_manager: Arc<TaskManager>,
    db_manager: Arc<AiosDBManager>,
}
```

#### ProgressManager - 进度管理器
```rust
pub struct ProgressManager {
    progress_channels: DashMap<String, broadcast::Sender<ProgressUpdate>>,
    current_tasks: DashMap<String, TaskProgress>,
}

pub struct TaskProgress {
    pub task_id: String,
    pub progress: f32,
    pub status: TaskStatus,
    pub message: String,
    pub start_time: DateTime<Utc>,
    pub estimated_completion: Option<DateTime<Utc>>,
}
```

#### MdbManager - MDB管理器
```rust
pub struct MdbManager {
    db_pool: Arc<Pool<MySql>>,
    cached_mdb_list: Arc<RwLock<Vec<MdbInfo>>>,
    last_update: Arc<RwLock<DateTime<Utc>>>,
}

pub struct MdbInfo {
    pub name: String,
    pub refno: u64,
    pub size: u64,
    pub created_at: DateTime<Utc>,
    pub db_files: Vec<DbFileInfo>,
}
```

#### TaskManager - 任务管理器
```rust
pub struct TaskManager {
    active_tasks: DashMap<String, TaskHandle>,
    task_queue: Arc<Mutex<VecDeque<TaskRequest>>>,
    max_concurrent_tasks: usize,
}

pub struct TaskHandle {
    pub id: String,
    pub handle: JoinHandle<Result<(), TaskError>>,
    pub cancel_token: CancellationToken,
    pub progress_sender: broadcast::Sender<ProgressUpdate>,
}
```

### 3. 服务集成

#### 在现有项目中集成GRPC服务
```rust
// src/grpc_service/mod.rs
pub mod progress_service;
pub mod server;

// 在main.rs中启动GRPC服务
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 现有的初始化代码...
    
    // 启动GRPC服务
    let grpc_server = tokio::spawn(async {
        start_grpc_server().await
    });
    
    // 现有的应用逻辑...
    run_app(None).await?;
    
    Ok(())
}
```

## Data Models

### 1. 进度数据模型

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressUpdate {
    pub task_id: String,
    pub progress: f32,
    pub status: TaskStatus,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    pub details: Option<ProgressDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressDetails {
    pub current_step: String,
    pub total_steps: u32,
    pub current_step_index: u32,
    pub processed_items: u64,
    pub total_items: u64,
    pub errors: Vec<String>,
}
```

### 2. MDB数据模型

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdbInfo {
    pub name: String,
    pub refno: u64,
    pub path: String,
    pub size: u64,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub db_files: Vec<DbFileInfo>,
    pub metadata: MdbMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbFileInfo {
    pub db_num: u32,
    pub name: String,
    pub size: u64,
    pub status: DbFileStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DbFileStatus {
    Available,
    Processing,
    Completed,
    Error(String),
}
```

### 3. 任务数据模型

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRequest {
    pub id: String,
    pub task_type: TaskType,
    pub mdb_name: String,
    pub db_files: Vec<u32>,
    pub options: TaskOptions,
    pub priority: TaskPriority,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskType {
    FullSync,
    IncrementalSync,
    ModelGeneration,
    SpatialTreeGeneration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOptions {
    pub enable_logging: bool,
    pub generate_models: bool,
    pub build_spatial_tree: bool,
    pub sync_team_data: bool,
}
```

## Error Handling

### 错误类型定义

```rust
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    
    #[error("Task error: {0}")]
    Task(String),
    
    #[error("MDB not found: {0}")]
    MdbNotFound(String),
    
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    
    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),
    
    #[error("Authentication failed")]
    AuthenticationFailed,
    
    #[error("Permission denied")]
    PermissionDenied,
}

impl From<ServiceError> for tonic::Status {
    fn from(err: ServiceError) -> Self {
        match err {
            ServiceError::Database(_) => {
                tonic::Status::internal("Database operation failed")
            }
            ServiceError::MdbNotFound(msg) => {
                tonic::Status::not_found(msg)
            }
            ServiceError::InvalidRequest(msg) => {
                tonic::Status::invalid_argument(msg)
            }
            ServiceError::AuthenticationFailed => {
                tonic::Status::unauthenticated("Authentication required")
            }
            ServiceError::PermissionDenied => {
                tonic::Status::permission_denied("Insufficient permissions")
            }
            _ => tonic::Status::internal("Internal server error"),
        }
    }
}
```

### 错误处理策略

1. **优雅降级**: 当部分功能不可用时，提供基本功能
2. **重试机制**: 对临时性错误实施指数退避重试
3. **断路器模式**: 防止级联故障
4. **详细日志**: 记录所有错误和异常情况

## Testing Strategy

### 1. 单元测试

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_progress_manager_create_task() {
        let manager = ProgressManager::new();
        let task_id = "test_task_1";
        
        manager.create_task(task_id.to_string()).await;
        
        assert!(manager.has_task(task_id));
    }
    
    #[tokio::test]
    async fn test_mdb_manager_get_list() {
        let manager = create_test_mdb_manager().await;
        let mdb_list = manager.get_mdb_list().await.unwrap();
        
        assert!(!mdb_list.is_empty());
    }
}
```

### 2. 集成测试

```rust
#[tokio::test]
async fn test_grpc_service_integration() {
    let server = start_test_server().await;
    let mut client = create_test_client().await;
    
    // 测试获取MDB列表
    let response = client.get_mdb_list(MdbListRequest {}).await.unwrap();
    assert!(!response.into_inner().mdbs.is_empty());
    
    // 测试启动任务
    let task_response = client.start_parse_task(StartTaskRequest {
        mdb_name: "test_mdb".to_string(),
        task_type: TaskType::FullSync as i32,
        options: Some(TaskOptions::default()),
    }).await.unwrap();
    
    assert!(!task_response.into_inner().task_id.is_empty());
}
```

### 3. 性能测试

```rust
#[tokio::test]
async fn test_concurrent_progress_streams() {
    let service = create_test_service().await;
    let mut handles = vec![];
    
    // 创建100个并发的进度流
    for i in 0..100 {
        let service_clone = service.clone();
        let handle = tokio::spawn(async move {
            let request = ProgressRequest {
                task_id: format!("task_{}", i),
            };
            
            let mut stream = service_clone
                .get_progress_stream(request)
                .await
                .unwrap()
                .into_inner();
                
            // 接收进度更新
            while let Some(update) = stream.next().await {
                // 处理进度更新
            }
        });
        handles.push(handle);
    }
    
    futures::future::join_all(handles).await;
}
```

### 4. 端到端测试

- 使用Docker Compose搭建完整的测试环境
- 模拟真实的MDB文件和解析场景
- 测试Web前端与GRPC服务的完整交互流程
- 验证错误处理和恢复机制

## Security Considerations

### 1. 认证和授权

```rust
pub struct AuthInterceptor {
    jwt_secret: String,
}

impl Interceptor for AuthInterceptor {
    fn call(&mut self, request: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        let token = request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "));
            
        match token {
            Some(token) => {
                if self.validate_token(token) {
                    Ok(request)
                } else {
                    Err(tonic::Status::unauthenticated("Invalid token"))
                }
            }
            None => Err(tonic::Status::unauthenticated("Missing token")),
        }
    }
}
```

### 2. 数据验证和清理

- 输入参数验证
- SQL注入防护
- XSS攻击防护
- 文件路径遍历防护

### 3. 速率限制

```rust
pub struct RateLimiter {
    requests: DashMap<String, VecDeque<Instant>>,
    max_requests: usize,
    window_duration: Duration,
}
```

## Deployment Strategy

### 1. 容器化部署

```dockerfile
# Dockerfile
FROM rust:1.70 as builder
WORKDIR /app
COPY . .
RUN cargo build --release --features grpc

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates
COPY --from=builder /app/target/release/aios-database /usr/local/bin/
EXPOSE 50051
CMD ["aios-database", "--grpc-server"]
```

### 2. Docker Compose配置

```yaml
version: '3.8'
services:
  grpc-service:
    build: .
    ports:
      - "50051:50051"
    environment:
      - DATABASE_URL=${DATABASE_URL}
      - RUST_LOG=info
    depends_on:
      - database
      
  grpc-gateway:
    image: envoyproxy/envoy:v1.27-latest
    ports:
      - "8080:8080"
    volumes:
      - ./envoy.yaml:/etc/envoy/envoy.yaml
    depends_on:
      - grpc-service
      
  frontend:
    build: ./frontend
    ports:
      - "3000:3000"
    depends_on:
      - grpc-gateway
```

### 3. 监控和日志

- 使用Prometheus进行指标收集
- 使用Grafana进行可视化监控
- 集成分布式链路追踪
- 结构化日志输出