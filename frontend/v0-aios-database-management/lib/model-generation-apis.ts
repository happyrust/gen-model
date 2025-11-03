import { buildApiUrl, handleResponse } from "./api"

// ============ 模型生成任务相关类型 ============

export interface ModelGenerationTask {
  id: string
  siteId: string
  projectName: string
  taskType: "model" | "mesh" | "spatial_tree"
  status: "pending" | "running" | "completed" | "failed" | "cancelled"
  progress: number
  startedAt?: string
  completedAt?: string
  duration?: number
  errorMessage?: string
  outputPath?: string
  fileSize?: number
}

export interface ModelGenerationTaskListParams {
  page?: number
  per_page?: number
  status?: string
  taskType?: string
  sort?: string
}

export interface ModelGenerationTaskListResponse {
  items: ModelGenerationTask[]
  total: number
  page: number
  per_page: number
  pages: number
}

export interface TaskStatusResponse {
  siteId: string
  status: "unknown" | "parsing" | "completed" | "failed"
  totalTasks: number
  completedTasks: number
  failedTasks: number
  runningTasks: number
  lastUpdateTime: string
}

export interface TaskDetail {
  id: string
  type: "parsing" | "model_generation"
  status: string
  progress: number
  logs: Array<{
    timestamp: string
    level: "info" | "warn" | "error"
    message: string
  }>
  metadata?: Record<string, unknown>
}

// ============ 模型生成任务相关 API ============

export async function fetchModelGenerationTasks(
  siteId: string,
  params?: ModelGenerationTaskListParams
) {
  const search = new URLSearchParams()
  if (params?.page) search.set("page", String(params.page))
  if (params?.per_page) search.set("per_page", String(params.per_page))
  if (params?.status) search.set("status", params.status)
  if (params?.taskType) search.set("taskType", params.taskType)
  if (params?.sort) search.set("sort", params.sort)

  const qs = search.toString()
  const url = qs
    ? `/api/model-generation/tasks?siteId=${siteId}&${qs}`
    : `/api/model-generation/tasks?siteId=${siteId}`

  const response = await fetch(buildApiUrl(url), {
    method: "GET",
    headers: {
      "Accept": "application/json",
    },
  })

  return handleResponse<ModelGenerationTaskListResponse>(response)
}

export async function fetchModelGenerationTaskStatus(siteId: string) {
  const response = await fetch(
    buildApiUrl(`/api/model-generation/status?siteId=${siteId}`),
    {
      method: "GET",
      headers: {
        "Accept": "application/json",
      },
    }
  )

  return handleResponse<TaskStatusResponse>(response)
}

export async function fetchTaskDetail(
  taskId: string,
  taskType: "parsing" | "model_generation"
) {
  const response = await fetch(
    buildApiUrl(`/api/tasks/${taskId}?type=${taskType}`),
    {
      method: "GET",
      headers: {
        "Accept": "application/json",
      },
    }
  )

  return handleResponse<TaskDetail>(response)
}

export async function retryTask(
  taskId: string,
  taskType: "parsing" | "model_generation"
) {
  const response = await fetch(buildApiUrl(`/api/tasks/${taskId}/retry`), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ type: taskType }),
  })

  return handleResponse<{ status: string; message: string }>(response)
}
