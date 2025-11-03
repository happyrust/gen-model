export interface Task {
  id: string
  name: string
  type: TaskType
  status: TaskStatus
  progress: number
  startTime: string
  endTime?: string
  duration?: number
  estimatedTime?: number
  priority: TaskPriority
  parameters: Record<string, any>
  result?: TaskResult
  error?: string
}

export type TaskType = 
  | 'ModelGeneration' 
  | 'SpatialTreeGeneration' 
  | 'FullSync' 
  | 'IncrementalSync'

export type TaskStatus = 
  | 'pending' 
  | 'running' 
  | 'paused' 
  | 'completed' 
  | 'failed' 
  | 'cancelled'

export type TaskPriority = 'Low' | 'Normal' | 'High' | 'Critical'

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

export interface SystemMetrics {
  cpu: number
  memory: number
  disk: number
  network: number
  uptime: number
  services: ServiceStatus[]
}

export interface ServiceStatus {
  name: string
  status: 'running' | 'stopped' | 'error'
  uptime: number
  lastCheck: string
}

export interface TaskQueue {
  pending: Task[]
  running: Task[]
  completed: Task[]
  failed: Task[]
}

export interface TaskAction {
  type: 'start' | 'stop' | 'pause' | 'resume' | 'cancel'
  taskId: string
  timestamp: string
}

export interface QueueAction {
  type: 'clear' | 'prioritize' | 'pause' | 'resume'
  taskIds?: string[]
  priority?: TaskPriority
}

export interface TaskMonitorState {
  tasks: Task[]
  systemMetrics: SystemMetrics
  isConnected: boolean
  lastUpdate: string
  error: string | null
}

export interface WebSocketMessage {
  type: 'task_update' | 'system_metrics' | 'queue_update'
  data: any
  timestamp: string
}
