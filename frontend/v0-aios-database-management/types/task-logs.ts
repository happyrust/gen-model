export interface LogEntry {
  id: string
  taskId: string
  level: LogLevel
  message: string
  timestamp: string
  source?: string
  metadata?: Record<string, any>
  stackTrace?: string
}

export type LogLevel = "info" | "warn" | "warning" | "error" | "debug" | "critical"

export interface LogFilters {
  level: 'all' | LogLevel
  search: string
  dateRange: [Date, Date] | null
  taskId: string
}

export interface PaginationState {
  currentPage: number
  pageSize: number
  totalPages: number
  totalItems: number
}

export interface LogSearchParams {
  query: string
  taskId?: string
  level?: LogLevel
  dateRange?: [Date, Date]
  page?: number
  pageSize?: number
}

export interface LogSearchResponse {
  logs: LogEntry[]
  total: number
  page: number
  pageSize: number
}

export interface LogExportParams {
  taskId: string
  format: 'txt' | 'json' | 'csv'
  dateRange: [Date, Date]
  level?: LogLevel
}

export interface LogExportResponse {
  downloadUrl: string
  filename: string
  size: number
}

export interface LogStatistics {
  totalLogs: number
  levelCounts: Record<LogLevel, number>
  timeRange: {
    start: string
    end: string
  }
  averageLogsPerHour: number
  errorRate: number
}

export interface LogSearchHistory {
  id: string
  query: string
  timestamp: string
  resultCount: number
}

export interface LogViewerState {
  logs: LogEntry[]
  loading: boolean
  error: string | null
  filters: LogFilters
  pagination: PaginationState
  searchHistory: LogSearchHistory[]
}
