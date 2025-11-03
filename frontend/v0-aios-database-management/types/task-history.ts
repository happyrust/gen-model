export interface TaskHistory {
  id: string
  taskId: string
  name: string
  type: TaskType
  status: TaskStatus
  startTime: string
  endTime: string
  duration: number
  result: TaskResult
  parameters: Record<string, any>
  logs: LogEntry[]
  createdAt: string
  updatedAt: string
}

export type TaskType = 
  | 'ModelGeneration' 
  | 'SpatialTreeGeneration' 
  | 'FullSync' 
  | 'IncrementalSync'

export type TaskStatus = 
  | 'completed' 
  | 'failed' 
  | 'cancelled' 
  | 'running'

export interface TaskResult {
  success: boolean
  message: string
  data?: any
  metrics?: TaskMetrics
}

export interface TaskMetrics {
  recordsProcessed: number
  processingTime: number
  memoryUsage: number
  cpuUsage: number
}

export interface LogEntry {
  id: string
  level: 'info' | 'warn' | 'error' | 'debug'
  message: string
  timestamp: string
  source?: string
}

export interface HistoryFilters {
  status: 'all' | TaskStatus
  type: 'all' | TaskType
  search: string
  dateRange: [Date, Date] | null
  sortBy: 'startTime' | 'endTime' | 'duration' | 'status'
  sortOrder: 'asc' | 'desc'
}

export interface PaginationState {
  currentPage: number
  pageSize: number
  totalPages: number
  totalItems: number
}

export interface TaskStatistics {
  total: number
  completed: number
  failed: number
  cancelled: number
  running: number
  successRate: number
  failureRate: number
  avgDuration: number
}

export interface TaskAnalytics {
  statistics: TaskStatistics
  charts: ChartData[]
  dateRange: [Date, Date]
  loading: boolean
}

export interface ChartData {
  date: string
  total: number
  completed: number
  failed: number
}

export interface TaskReplayParams {
  taskId: string
  parameters?: Record<string, any>
  priority?: TaskPriority
}

export type TaskPriority = 'Low' | 'Normal' | 'High' | 'Critical'

export interface TaskReplayResponse {
  success: boolean
  message: string
  newTaskId?: string
}

export interface HistoryExportParams {
  format: 'json' | 'csv' | 'xlsx'
  dateRange: [Date, Date]
  filters?: Partial<HistoryFilters>
}

export interface HistoryExportResponse {
  downloadUrl: string
  filename: string
  size: number
}
