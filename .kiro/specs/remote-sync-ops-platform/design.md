# Design Document

## Overview

本设计文档描述了异地协同运维平台的技术架构、组件设计和实现方案。该平台基于现有的异地同步后端功能，通过前端 Web UI 提供完整的部署、监控和运维能力。

### 设计目标

1. **统一管理** - 在一个界面中管理多个异地协同环境和站点
2. **实时监控** - 通过 SSE 实时推送同步状态和性能指标
3. **可视化** - 使用图表和流向图直观展示数据流向和系统状态
4. **易用性** - 提供向导式部署流程和一键式运维操作
5. **可扩展** - 支持添加新的监控指标和运维工具

### 技术栈

**前端**
- Next.js 14 (App Router)
- React 18
- TypeScript 5.x
- Tailwind CSS + shadcn/ui
- Recharts (图表库)
- React Flow (流向图)
- EventSource (SSE 客户端)

**后端**
- Rust + Axum
- SQLite (deployment_sites.sqlite)
- Tokio (异步运行时)
- rumqttc (MQTT 客户端)
- notify (文件监控)
- tokio::sync::broadcast (事件广播)

## Architecture

### 前端架构

```
app/
├── remote-sync/                    # 异地协同根路由
│   ├── page.tsx                    # 环境列表页
│   ├── deploy/                     # 部署向导
│   │   └── page.tsx
│   ├── monitor/                    # 监控仪表板
│   │   └── page.tsx
│   ├── flow/                       # 流向可视化
│   │   └── page.tsx
│   ├── logs/                       # 日志查询
│   │   └── page.tsx
│   ├── metrics/                    # 性能监控
│   │   └── page.tsx
│   ├── [envId]/                    # 环境详情
│   │   ├── page.tsx
│   │   └── sites/[siteId]/         # 站点详情
│   │       └── page.tsx
│   └── config/                     # 配置管理
│       └── page.tsx
```

### 后端架构

```
src/web_server/
├── remote_sync_handlers.rs         # 环境/站点 CRUD API
├── sync_control_handlers.rs        # 同步控制 API
├── sync_control_center.rs          # 同步控制中心
├── site_metadata.rs                # 元数据管理
├── remote_runtime.rs               # 运行时管理
└── sse_handlers.rs                 # SSE 事件流 (新增)
```

### 数据流架构

```
前端组件
    ↓ (HTTP/SSE)
API 路由层
    ↓
业务逻辑层 (Handlers)
    ↓
同步控制中心 (SyncControlCenter)
    ↓
运行时层 (Runtime + Watcher + MQTT)
    ↓
存储层 (SQLite + 文件系统)
```

## Components and Interfaces

### 1. 部署向导组件

#### DeployWizard Component

**功能**: 分步骤引导用户完成环境部署

**Props**:
```typescript
interface DeployWizardProps {
  onComplete: (envId: string) => void
  onCancel: () => void
}
```

**State**:
```typescript
interface DeployWizardState {
  currentStep: number  // 1-4
  envData: {
    name: string
    mqttHost: string
    mqttPort: number
    fileServerHost: string
    location: string
    locationDbs: string
  }
  sites: Array<{
    name: string
    httpHost: string
    dbnums: string
    location: string
  }>
  testResults: {
    mqttConnected: boolean
    httpReachable: boolean
    latency: number
  }
}
```

**子组件**:
- `StepBasicInfo` - 基本信息输入
- `StepSiteConfig` - 站点配置
- `StepConnectionTest` - 连接测试
- `StepActivation` - 激活确认


### 2. 监控仪表板组件

#### MonitorDashboard Component

**功能**: 实时显示同步状态和性能指标

**Props**:
```typescript
interface MonitorDashboardProps {
  envId?: string  // 可选，用于过滤特定环境
}
```

**State**:
```typescript
interface MonitorDashboardState {
  environments: Array<{
    id: string
    name: string
    status: 'running' | 'paused' | 'stopped'
    mqttConnected: boolean
    siteCount: number
    queueSize: number
  }>
  tasks: Array<SyncTask>
  metrics: {
    syncRate: number
    queueLength: number
    activeTasks: number
    cpuUsage: number
    memoryUsage: number
  }
  alerts: Array<Alert>
}
```

**SSE 连接**:
```typescript
useEffect(() => {
  const eventSource = new EventSource('/api/sync/events')
  
  eventSource.onmessage = (event) => {
    const data = JSON.parse(event.data)
    handleSyncEvent(data)
  }
  
  return () => eventSource.close()
}, [])
```

**子组件**:
- `EnvironmentCard` - 环境状态卡片
- `TaskList` - 任务列表
- `MetricsPanel` - 性能指标面板
- `AlertBanner` - 告警横幅

### 3. 流向可视化组件

#### FlowVisualization Component

**功能**: 使用力导向图展示数据流向

**Props**:
```typescript
interface FlowVisualizationProps {
  envId?: string
  timeRange: 'hour' | 'day' | 'week' | 'month'
}
```

**数据结构**:
```typescript
interface FlowData {
  nodes: Array<{
    id: string
    label: string
    type: 'env' | 'site'
    location: string
  }>
  edges: Array<{
    source: string
    target: string
    fileCount: number
    totalSize: number
    avgRate: number
    lastSync: string
  }>
}
```

**使用库**: React Flow 或 D3.js

**交互**:
- 鼠标悬停显示详情
- 点击节点高亮相关流向
- 拖拽调整布局
- 缩放和平移


### 4. 日志查询组件

#### LogQuery Component

**功能**: 多维度查询和展示同步日志

**Props**:
```typescript
interface LogQueryProps {
  defaultFilters?: LogFilters
}
```

**State**:
```typescript
interface LogQueryState {
  filters: {
    envId?: string
    siteId?: string
    status?: 'pending' | 'running' | 'completed' | 'failed'
    direction?: 'UPLOAD' | 'DOWNLOAD'
    startTime?: string
    endTime?: string
    keyword?: string
  }
  logs: Array<SyncLog>
  pagination: {
    page: number
    pageSize: number
    total: number
  }
  selectedLog?: SyncLog
}
```

**子组件**:
- `LogFilters` - 筛选条件表单
- `LogTable` - 日志表格
- `LogDetail` - 日志详情抽屉
- `LogExport` - 导出功能

### 5. 性能监控组件

#### MetricsMonitor Component

**功能**: 展示历史性能趋势和实时指标

**Props**:
```typescript
interface MetricsMonitorProps {
  envId?: string
  timeRange: 'hour' | 'day' | 'week' | 'month'
}
```

**State**:
```typescript
interface MetricsMonitorState {
  realtime: {
    syncRate: number
    queueLength: number
    activeTasks: number
    cpuUsage: number
    memoryUsage: number
  }
  history: Array<{
    timestamp: string
    syncRate: number
    queueLength: number
    activeTasks: number
  }>
  statistics: {
    p50: number
    p95: number
    p99: number
    avgSyncTime: number
    totalSynced: number
    totalFailed: number
  }
}
```

**图表类型**:
- 折线图 (Recharts LineChart) - 性能趋势
- 面积图 (Recharts AreaChart) - 队列长度
- 柱状图 (Recharts BarChart) - 成功/失败统计
- 仪表盘 (自定义) - 实时指标


### 6. 运维工具组件

#### OpsToolbar Component

**功能**: 提供常用运维操作的快捷入口

**Props**:
```typescript
interface OpsToolbarProps {
  envId: string
  onOperationComplete: () => void
}
```

**操作列表**:
```typescript
interface OpsOperation {
  id: string
  label: string
  icon: React.ReactNode
  action: () => Promise<void>
  confirmRequired: boolean
  confirmMessage?: string
}

const operations: OpsOperation[] = [
  {
    id: 'start',
    label: '启动同步',
    icon: <Play />,
    action: () => startSync(envId),
    confirmRequired: false
  },
  {
    id: 'stop',
    label: '停止同步',
    icon: <Stop />,
    action: () => stopSync(envId),
    confirmRequired: true,
    confirmMessage: '确定要停止同步服务吗？运行中的任务将等待完成。'
  },
  {
    id: 'pause',
    label: '暂停同步',
    icon: <Pause />,
    action: () => pauseSync(envId),
    confirmRequired: false
  },
  {
    id: 'clear-queue',
    label: '清空队列',
    icon: <Trash />,
    action: () => clearQueue(envId),
    confirmRequired: true,
    confirmMessage: '确定要清空任务队列吗？所有待处理任务将被删除。'
  }
]
```

### 7. 配置管理组件

#### ConfigManager Component

**功能**: 管理同步系统的配置参数

**Props**:
```typescript
interface ConfigManagerProps {
  envId?: string
}
```

**State**:
```typescript
interface ConfigManagerState {
  config: {
    autoRetry: boolean
    maxRetries: number
    retryDelayMs: number
    maxConcurrentSyncs: number
    batchSize: number
    syncIntervalMs: number
  }
  modified: boolean
  saving: boolean
}
```

**验证规则**:
```typescript
const validationRules = {
  maxRetries: { min: 0, max: 10 },
  retryDelayMs: { min: 1000, max: 60000 },
  maxConcurrentSyncs: { min: 1, max: 20 },
  batchSize: { min: 1, max: 100 },
  syncIntervalMs: { min: 100, max: 10000 }
}
```


## Data Models

### 前端数据模型

#### Environment (扩展)
```typescript
interface Environment {
  id: string
  name: string
  mqttHost: string
  mqttPort: number
  fileServerHost: string
  location: string
  locationDbs: string
  reconnectInitialMs: number
  reconnectMaxMs: number
  createdAt: string
  updatedAt: string
  
  // 运行时状态 (从 API 获取)
  status: 'running' | 'paused' | 'stopped'
  mqttConnected: boolean
  watcherActive: boolean
  siteCount: number
  queueSize: number
}
```

#### Site (扩展)
```typescript
interface Site {
  id: string
  envId: string
  name: string
  location: string
  httpHost: string
  dbnums: string
  notes: string
  createdAt: string
  updatedAt: string
  
  // 运行时状态
  reachable: boolean
  latency: number
  lastSync: string
  syncCount: number
  failCount: number
}
```

#### SyncTask
```typescript
interface SyncTask {
  id: string
  fileName: string
  filePath: string
  fileSize: number
  fileHash: string
  recordCount: number
  envId: string
  sourceEnv: string
  targetSite: string
  direction: 'UPLOAD' | 'DOWNLOAD'
  status: 'pending' | 'running' | 'completed' | 'failed' | 'cancelled'
  priority: number
  retryCount: number
  progress: number  // 0-100
  createdAt: string
  startedAt?: string
  completedAt?: string
  errorMessage?: string
}
```

#### SyncLog
```typescript
interface SyncLog {
  id: string
  taskId: string
  envId: string
  sourceEnv: string
  targetSite: string
  siteId: string
  direction: 'UPLOAD' | 'DOWNLOAD'
  filePath: string
  fileSize: number
  recordCount: number
  status: 'pending' | 'running' | 'completed' | 'failed'
  errorMessage?: string
  notes?: string
  startedAt: string
  completedAt?: string
  createdAt: string
  updatedAt: string
}
```


#### Metrics
```typescript
interface Metrics {
  syncRate: number          // MB/s
  queueLength: number
  activeTasks: number
  cpuUsage: number         // 0-100
  memoryUsage: number      // 0-100
  totalSynced: number
  totalFailed: number
  avgSyncTime: number      // ms
  p50: number              // ms
  p95: number              // ms
  p99: number              // ms
}
```

#### Alert
```typescript
interface Alert {
  id: string
  type: 'warning' | 'error' | 'info'
  title: string
  message: string
  envId?: string
  siteId?: string
  timestamp: string
  acknowledged: boolean
  actionUrl?: string
}
```

#### FlowNode
```typescript
interface FlowNode {
  id: string
  label: string
  type: 'env' | 'site'
  location: string
  position: { x: number; y: number }
  data: {
    status: 'active' | 'inactive'
    queueSize: number
    syncCount: number
  }
}
```

#### FlowEdge
```typescript
interface FlowEdge {
  id: string
  source: string
  target: string
  label: string
  data: {
    fileCount: number
    totalSize: number
    avgRate: number
    lastSync: string
    hasWarning: boolean
  }
  animated: boolean
}
```

### 后端数据模型 (新增/扩展)

#### SyncEvent (SSE 事件)
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncEvent {
    Started {
        env_id: String,
        timestamp: SystemTime,
    },
    Stopped {
        env_id: String,
        timestamp: SystemTime,
    },
    Paused {
        env_id: String,
        timestamp: SystemTime,
    },
    Resumed {
        env_id: String,
        timestamp: SystemTime,
    },
    SyncStarted {
        task_id: String,
        file_path: String,
        file_size: u64,
        timestamp: SystemTime,
    },
    SyncProgress {
        task_id: String,
        progress: u8,  // 0-100
        timestamp: SystemTime,
    },
    SyncCompleted {
        task_id: String,
        file_path: String,
        duration_ms: u64,
        timestamp: SystemTime,
    },
    SyncFailed {
        task_id: String,
        file_path: String,
        error: String,
        timestamp: SystemTime,
    },
    MqttConnected {
        env_id: String,
        timestamp: SystemTime,
    },
    MqttDisconnected {
        env_id: String,
        reason: String,
        timestamp: SystemTime,
    },
    QueueSizeChanged {
        env_id: String,
        queue_size: u32,
        timestamp: SystemTime,
    },
    MetricsUpdated {
        env_id: String,
        metrics: Metrics,
        timestamp: SystemTime,
    },
}
```


## Error Handling

### 前端错误处理策略

#### 1. API 调用错误
```typescript
async function apiCall<T>(
  url: string,
  options?: RequestInit
): Promise<T> {
  try {
    const response = await fetch(url, {
      ...options,
      headers: {
        'Content-Type': 'application/json',
        ...options?.headers,
      },
    })
    
    if (!response.ok) {
      const error = await response.json()
      throw new ApiError(
        error.message || 'API 调用失败',
        response.status,
        error
      )
    }
    
    return await response.json()
  } catch (error) {
    if (error instanceof ApiError) {
      throw error
    }
    throw new NetworkError('网络连接失败，请检查网络设置')
  }
}
```

#### 2. SSE 连接错误
```typescript
function useSSE(url: string) {
  const [connected, setConnected] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const reconnectTimeoutRef = useRef<NodeJS.Timeout>()
  
  useEffect(() => {
    let eventSource: EventSource | null = null
    let reconnectAttempts = 0
    const maxReconnectAttempts = 5
    
    function connect() {
      eventSource = new EventSource(url)
      
      eventSource.onopen = () => {
        setConnected(true)
        setError(null)
        reconnectAttempts = 0
      }
      
      eventSource.onerror = () => {
        setConnected(false)
        eventSource?.close()
        
        if (reconnectAttempts < maxReconnectAttempts) {
          const delay = Math.min(1000 * Math.pow(2, reconnectAttempts), 30000)
          reconnectTimeoutRef.current = setTimeout(() => {
            reconnectAttempts++
            connect()
          }, delay)
        } else {
          setError('SSE 连接失败，已达到最大重试次数')
        }
      }
    }
    
    connect()
    
    return () => {
      eventSource?.close()
      if (reconnectTimeoutRef.current) {
        clearTimeout(reconnectTimeoutRef.current)
      }
    }
  }, [url])
  
  return { connected, error }
}
```

#### 3. 表单验证错误
```typescript
function validateEnvForm(data: EnvFormData): ValidationResult {
  const errors: Record<string, string> = {}
  
  if (!data.name || data.name.trim().length === 0) {
    errors.name = '环境名称不能为空'
  }
  
  if (data.mqttHost && !isValidHost(data.mqttHost)) {
    errors.mqttHost = 'MQTT 主机地址格式不正确'
  }
  
  if (data.mqttPort && (data.mqttPort < 1 || data.mqttPort > 65535)) {
    errors.mqttPort = '端口号必须在 1-65535 之间'
  }
  
  if (data.fileServerHost && !isValidUrl(data.fileServerHost)) {
    errors.fileServerHost = '文件服务器地址格式不正确'
  }
  
  return {
    valid: Object.keys(errors).length === 0,
    errors
  }
}
```


### 后端错误处理策略

#### 1. API 错误响应
```rust
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub status: String,  // "error"
    pub message: String,
    pub code: Option<String>,
    pub details: Option<serde_json::Value>,
}

impl ErrorResponse {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            message: message.into(),
            code: None,
            details: None,
        }
    }
    
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }
    
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

// 使用示例
async fn activate_env(
    Path(env_id): Path<String>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match activate_environment(&env_id).await {
        Ok(result) => Ok(Json(SuccessResponse::new(result))),
        Err(e) => {
            let error = ErrorResponse::new(e.to_string())
                .with_code("ENV_ACTIVATION_FAILED");
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error)))
        }
    }
}
```

#### 2. 同步任务错误处理
```rust
async fn process_sync_task(task: &mut SyncTask) -> Result<(), SyncError> {
    // 重试逻辑
    let max_retries = 3;
    let mut last_error = None;
    
    for attempt in 0..=max_retries {
        match execute_sync(task).await {
            Ok(_) => {
                task.status = SyncTaskStatus::Completed;
                return Ok(());
            }
            Err(e) => {
                last_error = Some(e);
                task.retry_count = attempt + 1;
                
                if attempt < max_retries {
                    let delay = Duration::from_millis(1000 * 2_u64.pow(attempt));
                    tokio::time::sleep(delay).await;
                } else {
                    task.status = SyncTaskStatus::Failed;
                    task.error_message = Some(last_error.unwrap().to_string());
                    return Err(last_error.unwrap());
                }
            }
        }
    }
    
    unreachable!()
}
```

#### 3. MQTT 连接错误处理
```rust
async fn handle_mqtt_connection(
    mqtt_config: &MqttConfig,
) -> Result<(), MqttError> {
    let mut reconnect_attempts = 0;
    let max_reconnect_attempts = 10;
    
    loop {
        match connect_mqtt(mqtt_config).await {
            Ok(client) => {
                reconnect_attempts = 0;
                // 处理消息...
                return Ok(());
            }
            Err(e) => {
                reconnect_attempts += 1;
                
                if reconnect_attempts >= max_reconnect_attempts {
                    return Err(MqttError::MaxReconnectAttemptsReached);
                }
                
                let delay = std::cmp::min(
                    mqtt_config.reconnect_initial_ms * 2_u64.pow(reconnect_attempts),
                    mqtt_config.reconnect_max_ms
                );
                
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
    }
}
```


## Testing Strategy

### 前端测试

#### 1. 单元测试 (Jest + React Testing Library)
```typescript
// components/__tests__/DeployWizard.test.tsx
describe('DeployWizard', () => {
  it('should render all steps', () => {
    render(<DeployWizard onComplete={jest.fn()} onCancel={jest.fn()} />)
    expect(screen.getByText('步骤 1 / 4')).toBeInTheDocument()
  })
  
  it('should validate form inputs', async () => {
    render(<DeployWizard onComplete={jest.fn()} onCancel={jest.fn()} />)
    
    const nameInput = screen.getByLabelText('环境名称')
    fireEvent.change(nameInput, { target: { value: '' } })
    fireEvent.blur(nameInput)
    
    expect(await screen.findByText('环境名称不能为空')).toBeInTheDocument()
  })
  
  it('should call onComplete when wizard finishes', async () => {
    const onComplete = jest.fn()
    render(<DeployWizard onComplete={onComplete} onCancel={jest.fn()} />)
    
    // 填写表单并提交...
    
    await waitFor(() => {
      expect(onComplete).toHaveBeenCalledWith(expect.any(String))
    })
  })
})
```

#### 2. 集成测试
```typescript
// __tests__/integration/deploy-flow.test.tsx
describe('Deploy Flow Integration', () => {
  it('should complete full deployment flow', async () => {
    // Mock API responses
    server.use(
      rest.post('/api/remote-sync/envs', (req, res, ctx) => {
        return res(ctx.json({ status: 'success', id: 'env-123' }))
      }),
      rest.post('/api/remote-sync/envs/:id/activate', (req, res, ctx) => {
        return res(ctx.json({ status: 'success' }))
      })
    )
    
    render(<DeployWizard onComplete={jest.fn()} onCancel={jest.fn()} />)
    
    // Step 1: Basic Info
    fireEvent.change(screen.getByLabelText('环境名称'), {
      target: { value: '测试环境' }
    })
    fireEvent.click(screen.getByText('下一步'))
    
    // Step 2: Site Config
    fireEvent.click(screen.getByText('添加站点'))
    // ...
    
    // Step 3: Connection Test
    fireEvent.click(screen.getByText('测试连接'))
    await waitFor(() => {
      expect(screen.getByText('连接成功')).toBeInTheDocument()
    })
    
    // Step 4: Activation
    fireEvent.click(screen.getByText('激活环境'))
    await waitFor(() => {
      expect(screen.getByText('激活成功')).toBeInTheDocument()
    })
  })
})
```

#### 3. E2E 测试 (Playwright)
```typescript
// e2e/deploy-environment.spec.ts
test('deploy new environment', async ({ page }) => {
  await page.goto('/remote-sync/deploy')
  
  // Fill basic info
  await page.fill('[name="name"]', '测试环境')
  await page.fill('[name="mqttHost"]', 'mqtt.example.com')
  await page.click('button:has-text("下一步")')
  
  // Add site
  await page.click('button:has-text("添加站点")')
  await page.fill('[name="siteName"]', '测试站点')
  await page.fill('[name="httpHost"]', 'http://test.example.com')
  await page.click('button:has-text("确认")')
  await page.click('button:has-text("下一步")')
  
  // Test connection
  await page.click('button:has-text("测试连接")')
  await expect(page.locator('text=连接成功')).toBeVisible()
  await page.click('button:has-text("下一步")')
  
  // Activate
  await page.click('button:has-text("激活环境")')
  await expect(page.locator('text=激活成功')).toBeVisible()
  
  // Verify redirect to monitor page
  await expect(page).toHaveURL(/\/remote-sync\/monitor/)
})
```


### 后端测试

#### 1. 单元测试
```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_create_environment() {
        let db = setup_test_db().await;
        
        let env = RemoteSyncEnv {
            id: "test-env".to_string(),
            name: "测试环境".to_string(),
            mqtt_host: Some("mqtt.example.com".to_string()),
            mqtt_port: Some(1883),
            // ...
        };
        
        let result = create_env(&db, &env).await;
        assert!(result.is_ok());
        
        let loaded = get_env(&db, "test-env").await.unwrap();
        assert_eq!(loaded.name, "测试环境");
    }
    
    #[tokio::test]
    async fn test_sync_task_retry() {
        let mut task = SyncTask {
            id: "task-1".to_string(),
            status: SyncTaskStatus::Pending,
            retry_count: 0,
            // ...
        };
        
        // Simulate failure
        let result = process_sync_task(&mut task).await;
        assert!(result.is_err());
        assert_eq!(task.retry_count, 3);
        assert_eq!(task.status, SyncTaskStatus::Failed);
    }
}
```

#### 2. 集成测试
```rust
#[cfg(test)]
mod integration_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    
    #[tokio::test]
    async fn test_deploy_flow() {
        let app = create_test_app().await;
        
        // Create environment
        let create_req = Request::builder()
            .method("POST")
            .uri("/api/remote-sync/envs")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"测试环境","mqtt_host":"mqtt.example.com"}"#))
            .unwrap();
        
        let response = app.clone().oneshot(create_req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        
        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let env_id = result["id"].as_str().unwrap();
        
        // Activate environment
        let activate_req = Request::builder()
            .method("POST")
            .uri(format!("/api/remote-sync/envs/{}/activate", env_id))
            .body(Body::empty())
            .unwrap();
        
        let response = app.oneshot(activate_req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
```

#### 3. 性能测试
```rust
#[cfg(test)]
mod performance_tests {
    use super::*;
    use std::time::Instant;
    
    #[tokio::test]
    async fn test_concurrent_sync_tasks() {
        let center = SyncControlCenter::new();
        let start = Instant::now();
        
        // Create 100 tasks
        let mut handles = vec![];
        for i in 0..100 {
            let task_id = center.add_task(NewSyncTaskParams {
                file_path: format!("/test/file_{}.cba", i),
                file_size: 1024 * 1024,
                priority: 5,
                // ...
            }).await;
            
            handles.push(tokio::spawn(async move {
                // Simulate sync
                tokio::time::sleep(Duration::from_millis(100)).await;
            }));
        }
        
        for handle in handles {
            handle.await.unwrap();
        }
        
        let duration = start.elapsed();
        assert!(duration.as_secs() < 5, "Should complete within 5 seconds");
    }
}
```

## Performance Optimization

### 前端优化

#### 1. 代码分割
```typescript
// app/remote-sync/layout.tsx
import dynamic from 'next/dynamic'

const MonitorDashboard = dynamic(() => import('./monitor/page'), {
  loading: () => <LoadingSpinner />,
  ssr: false
})

const FlowVisualization = dynamic(() => import('./flow/page'), {
  loading: () => <LoadingSpinner />,
  ssr: false
})
```

#### 2. 数据缓存
```typescript
// lib/api/cache.ts
import { useQuery } from '@tanstack/react-query'

export function useEnvironments() {
  return useQuery({
    queryKey: ['environments'],
    queryFn: fetchEnvironments,
    staleTime: 30000,  // 30 seconds
    cacheTime: 300000,  // 5 minutes
  })
}

export function useSyncTasks(envId: string) {
  return useQuery({
    queryKey: ['sync-tasks', envId],
    queryFn: () => fetchSyncTasks(envId),
    refetchInterval: 5000,  // Refetch every 5 seconds
  })
}
```

#### 3. 虚拟滚动
```typescript
// components/LogTable.tsx
import { useVirtualizer } from '@tanstack/react-virtual'

function LogTable({ logs }: { logs: SyncLog[] }) {
  const parentRef = useRef<HTMLDivElement>(null)
  
  const virtualizer = useVirtualizer({
    count: logs.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 50,
    overscan: 10,
  })
  
  return (
    <div ref={parentRef} style={{ height: '600px', overflow: 'auto' }}>
      <div style={{ height: `${virtualizer.getTotalSize()}px` }}>
        {virtualizer.getVirtualItems().map((virtualRow) => (
          <LogRow
            key={virtualRow.index}
            log={logs[virtualRow.index]}
            style={{
              position: 'absolute',
              top: 0,
              left: 0,
              width: '100%',
              height: `${virtualRow.size}px`,
              transform: `translateY(${virtualRow.start}px)`,
            }}
          />
        ))}
      </div>
    </div>
  )
}
```


### 后端优化

#### 1. 连接池
```rust
// 数据库连接池
use sqlx::sqlite::SqlitePoolOptions;

pub async fn create_db_pool(db_path: &str) -> Result<SqlitePool, sqlx::Error> {
    SqlitePoolOptions::new()
        .max_connections(10)
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(db_path)
        .await
}
```

#### 2. 批量操作
```rust
// 批量插入日志
pub async fn batch_insert_logs(
    pool: &SqlitePool,
    logs: &[SyncLog],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    
    for chunk in logs.chunks(100) {
        let mut query_builder = QueryBuilder::new(
            "INSERT INTO remote_sync_logs (id, task_id, env_id, ...)"
        );
        
        query_builder.push_values(chunk, |mut b, log| {
            b.push_bind(&log.id)
             .push_bind(&log.task_id)
             .push_bind(&log.env_id);
            // ...
        });
        
        query_builder.build().execute(&mut *tx).await?;
    }
    
    tx.commit().await?;
    Ok(())
}
```

#### 3. 异步任务队列
```rust
// 使用 tokio 的 mpsc 通道实现任务队列
use tokio::sync::mpsc;

pub struct TaskQueue {
    tx: mpsc::Sender<SyncTask>,
    rx: mpsc::Receiver<SyncTask>,
}

impl TaskQueue {
    pub fn new(capacity: usize) -> Self {
        let (tx, rx) = mpsc::channel(capacity);
        Self { tx, rx }
    }
    
    pub async fn enqueue(&self, task: SyncTask) -> Result<(), SendError<SyncTask>> {
        self.tx.send(task).await
    }
    
    pub async fn dequeue(&mut self) -> Option<SyncTask> {
        self.rx.recv().await
    }
}

// 工作线程
pub async fn worker(mut queue: TaskQueue) {
    while let Some(task) = queue.dequeue().await {
        tokio::spawn(async move {
            process_sync_task(&task).await;
        });
    }
}
```

## Security Considerations

### 1. API 认证
```rust
// 使用 JWT 进行 API 认证
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
    role: String,
}

async fn verify_token(
    TypedHeader(authorization): TypedHeader<Authorization<Bearer>>,
) -> Result<Claims, StatusCode> {
    let token = authorization.token();
    
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(SECRET_KEY),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|_| StatusCode::UNAUTHORIZED)
}
```

### 2. 输入验证
```rust
// 验证环境配置输入
pub fn validate_env_config(env: &RemoteSyncEnv) -> Result<(), ValidationError> {
    if env.name.trim().is_empty() {
        return Err(ValidationError::new("name", "环境名称不能为空"));
    }
    
    if let Some(ref host) = env.mqtt_host {
        if !is_valid_hostname(host) {
            return Err(ValidationError::new("mqtt_host", "MQTT 主机地址格式不正确"));
        }
    }
    
    if let Some(port) = env.mqtt_port {
        if port == 0 || port > 65535 {
            return Err(ValidationError::new("mqtt_port", "端口号必须在 1-65535 之间"));
        }
    }
    
    Ok(())
}
```

### 3. 文件路径安全
```rust
// 防止路径遍历攻击
use std::path::{Path, PathBuf};

pub fn sanitize_file_path(base: &Path, relative: &str) -> Result<PathBuf, SecurityError> {
    let path = base.join(relative);
    let canonical = path.canonicalize()
        .map_err(|_| SecurityError::InvalidPath)?;
    
    if !canonical.starts_with(base) {
        return Err(SecurityError::PathTraversal);
    }
    
    Ok(canonical)
}
```

### 4. 速率限制
```rust
// 使用 tower 的 rate limit 中间件
use tower::limit::RateLimitLayer;
use tower::ServiceBuilder;

let app = Router::new()
    .route("/api/remote-sync/envs", post(create_env))
    .layer(
        ServiceBuilder::new()
            .layer(RateLimitLayer::new(100, Duration::from_secs(60)))
    );
```

## Deployment

### 前端部署

#### 1. 构建配置
```javascript
// next.config.mjs
/** @type {import('next').NextConfig} */
const nextConfig = {
  output: 'standalone',
  env: {
    NEXT_PUBLIC_API_BASE_URL: process.env.NEXT_PUBLIC_API_BASE_URL,
  },
  experimental: {
    serverActions: true,
  },
}

export default nextConfig
```

#### 2. Docker 部署
```dockerfile
# Dockerfile
FROM node:20-alpine AS builder
WORKDIR /app
COPY package*.json ./
RUN npm ci
COPY . .
RUN npm run build

FROM node:20-alpine AS runner
WORKDIR /app
ENV NODE_ENV production
COPY --from=builder /app/.next/standalone ./
COPY --from=builder /app/.next/static ./.next/static
COPY --from=builder /app/public ./public
EXPOSE 3000
CMD ["node", "server.js"]
```

### 后端部署

#### 1. 编译配置
```toml
# Cargo.toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
```

#### 2. systemd 服务
```ini
# /etc/systemd/system/remote-sync-ops.service
[Unit]
Description=Remote Sync Ops Platform
After=network.target

[Service]
Type=simple
User=app
WorkingDirectory=/opt/remote-sync-ops
ExecStart=/opt/remote-sync-ops/web_server
Restart=on-failure
RestartSec=5s
Environment="RUST_LOG=info"

[Install]
WantedBy=multi-user.target
```

## Monitoring and Logging

### 前端监控

#### 1. 错误追踪 (Sentry)
```typescript
// lib/sentry.ts
import * as Sentry from '@sentry/nextjs'

Sentry.init({
  dsn: process.env.NEXT_PUBLIC_SENTRY_DSN,
  environment: process.env.NODE_ENV,
  tracesSampleRate: 0.1,
})
```

#### 2. 性能监控
```typescript
// lib/analytics.ts
export function trackPageView(url: string) {
  if (typeof window !== 'undefined' && window.gtag) {
    window.gtag('config', GA_TRACKING_ID, {
      page_path: url,
    })
  }
}

export function trackEvent(action: string, category: string, label?: string) {
  if (typeof window !== 'undefined' && window.gtag) {
    window.gtag('event', action, {
      event_category: category,
      event_label: label,
    })
  }
}
```

### 后端监控

#### 1. 日志记录
```rust
use tracing::{info, warn, error};
use tracing_subscriber;

pub fn init_logging() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .init();
}

// 使用示例
info!("Environment activated: {}", env_id);
warn!("MQTT connection lost, attempting reconnect");
error!("Sync task failed: {}", error);
```

#### 2. Prometheus 指标
```rust
use prometheus::{Counter, Gauge, Histogram, Registry};

lazy_static! {
    static ref SYNC_TASKS_TOTAL: Counter = Counter::new(
        "sync_tasks_total",
        "Total number of sync tasks"
    ).unwrap();
    
    static ref SYNC_TASKS_FAILED: Counter = Counter::new(
        "sync_tasks_failed",
        "Number of failed sync tasks"
    ).unwrap();
    
    static ref SYNC_DURATION: Histogram = Histogram::new(
        "sync_duration_seconds",
        "Sync task duration in seconds"
    ).unwrap();
    
    static ref QUEUE_SIZE: Gauge = Gauge::new(
        "queue_size",
        "Current queue size"
    ).unwrap();
}

// 导出指标
async fn metrics_handler() -> String {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    encoder.encode_to_string(&metric_families).unwrap()
}
```
