export interface TaskCreationRequest {
  taskName: string
  taskType: TaskType
  siteId: string
  priority: TaskPriority
  description?: string
  parameters: TaskParameters
}

export type TaskType = 
  | 'DataParsingWizard'  // 数据解析任务
  | 'ModelGeneration'     // 模型生成任务
  | 'SpatialTreeGeneration' // 空间树生成任务
  | 'FullSync'            // 全量同步任务
  | 'IncrementalSync'     // 增量同步任务

export type TaskPriority = 'Low' | 'Normal' | 'High' | 'Critical'

export interface TaskParameters {
  // 解析任务参数
  parseMode?: 'all' | 'dbnum' | 'refno'
  dbnum?: number
  refno?: string
  
  // 模型生成参数
  generateModels?: boolean
  generateMesh?: boolean
  generateSpatialTree?: boolean
  applyBooleanOperation?: boolean
  meshTolRatio?: number
  
  // 同步任务参数
  syncMode?: 'full' | 'incremental'
  targetSesno?: number
  
  // 通用参数
  maxConcurrent?: number
  parallelProcessing?: boolean
}

export interface TaskCreationResponse {
  success: boolean
  taskId: string
  message: string
  error?: string
}

export interface DeploymentSite {
  id: string
  name: string
  status: string
  environment: string
  description?: string
  config?: any
}

export interface TaskTemplate {
  id: string
  name: string
  description: string
  taskType: TaskType
  defaultParameters: Partial<TaskParameters>
  estimatedDuration?: number
}

export interface TaskCreationFormData {
  taskName: string
  taskType: TaskType
  siteId: string
  priority: TaskPriority
  description: string
  parameters: TaskParameters
}







