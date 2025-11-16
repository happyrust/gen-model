// 异地协同运维平台数据模型

/**
 * 环境 (扩展)
 */
export interface Environment {
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
  status?: 'running' | 'paused' | 'stopped'
  mqttConnected?: boolean
  watcherActive?: boolean
  siteCount?: number
  queueSize?: number
}

/**
 * 站点 (扩展)
 */
export interface Site {
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
  reachable?: boolean
  latency?: number
  lastSync?: string
  syncCount?: number
  failCount?: number
}

/**
 * 同步任务状态
 */
export type SyncTaskStatus = 'pending' | 'running' | 'completed' | 'failed' | 'cancelled'

/**
 * 同步任务
 */
export interface SyncTask {
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
  status: SyncTaskStatus
  priority: number
  retryCount: number
  progress: number  // 0-100
  createdAt: string
  startedAt?: string
  completedAt?: string
  errorMessage?: string
}

/**
 * 同步日志
 */
export interface SyncLog {
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

/**
 * 性能指标
 */
export interface Metrics {
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

/**
 * 告警类型
 */
export type AlertType = 'warning' | 'error' | 'info'

/**
 * 告警
 */
export interface Alert {
  id: string
  type: AlertType
  title: string
  message: string
  envId?: string
  siteId?: string
  timestamp: string
  acknowledged: boolean
  actionUrl?: string
}

/**
 * 流向图节点
 */
export interface FlowNode {
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

/**
 * 流向图边
 */
export interface FlowEdge {
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

/**
 * SSE 事件类型
 */
export type SyncEventType = 
  | 'Started'
  | 'Stopped'
  | 'Paused'
  | 'Resumed'
  | 'SyncStarted'
  | 'SyncProgress'
  | 'SyncCompleted'
  | 'SyncFailed'
  | 'MqttConnected'
  | 'MqttDisconnected'
  | 'QueueSizeChanged'
  | 'MetricsUpdated'

/**
 * SSE 事件
 */
export interface SyncEvent {
  type: SyncEventType
  data: {
    env_id?: string
    task_id?: string
    file_path?: string
    file_size?: number
    progress?: number
    duration_ms?: number
    error?: string
    reason?: string
    queue_size?: number
    metrics?: Metrics
    timestamp: string
  }
}

/**
 * 同步配置
 */
export interface SyncConfig {
  autoRetry: boolean
  maxRetries: number
  retryDelayMs: number
  maxConcurrentSyncs: number
  batchSize: number
  syncIntervalMs: number
}

/**
 * 站点元数据条目
 */
export interface SiteMetadataEntry {
  fileName: string
  filePath: string
  fileSize: number
  fileHash: string
  recordCount: number
  direction: 'UPLOAD' | 'DOWNLOAD'
  sourceEnv: string
  downloadUrl: string
  relativePath: string
  updatedAt: string
}

/**
 * 站点元数据
 */
export interface SiteMetadata {
  envId: string
  envName: string
  siteId: string
  siteName: string
  siteHttpHost: string
  generatedAt: string
  entries: SiteMetadataEntry[]
}

/**
 * 元数据响应
 */
export interface MetadataResponse {
  status: string
  source: 'local_path' | 'http' | 'cache'
  fetchedAt: string
  entryCount: number
  cachePath?: string
  httpBase?: string
  localBase?: string
  warnings: string[]
  env: {
    id: string
    name: string
    fileHost?: string
  }
  site: {
    id: string
    name: string
    host?: string
  }
  metadata: SiteMetadata
}

/**
 * 流向统计
 */
export interface FlowStatistics {
  sourceEnv: string
  targetSite: string
  fileCount: number
  totalSize: number
  avgRate: number
  lastSync: string
}

/**
 * 每日统计
 */
export interface DailyStatistics {
  day: string
  total: number
  completed: number
  failed: number
  recordCount: number
  totalBytes: number
}

/**
 * API 响应基础类型
 */
export interface ApiResponse<T = any> {
  status: 'success' | 'error'
  message?: string
  item?: T
  items?: T[]
  total?: number
}

/**
 * 分页参数
 */
export interface PaginationParams {
  limit?: number
  offset?: number
}

/**
 * 日志查询参数
 */
export interface LogQueryParams extends PaginationParams {
  envId?: string
  siteId?: string
  targetSite?: string
  status?: SyncTaskStatus
  direction?: 'UPLOAD' | 'DOWNLOAD'
  start?: string
  end?: string
  keyword?: string
}

/**
 * 统计查询参数
 */
export interface StatsQueryParams {
  envId?: string
  days?: number
  limit?: number
}
