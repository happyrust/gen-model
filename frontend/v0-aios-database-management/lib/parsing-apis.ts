import { buildApiUrl, handleResponse } from "./api"

// ============ 解析任务相关类型 ============

export interface ParsingTask {
  id: string
  siteId: string
  projectName: string
  status: "pending" | "running" | "completed" | "failed" | "cancelled"
  progress: number
  startedAt?: string
  completedAt?: string
  duration?: number
  errorMessage?: string
  filesProcessed?: number
  filesTotal?: number
}

export interface ParsingTaskListParams {
  page?: number
  per_page?: number
  status?: string
  sort?: string
}

export interface ParsingTaskListResponse {
  items: ParsingTask[]
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

// ============ 解析任务相关 API ============

export async function fetchParsingTasks(
  siteId: string,
  params?: ParsingTaskListParams
) {
  const search = new URLSearchParams()
  if (params?.page) search.set("page", String(params.page))
  if (params?.per_page) search.set("per_page", String(params.per_page))
  if (params?.status) search.set("status", params.status)
  if (params?.sort) search.set("sort", params.sort)

  const qs = search.toString()
  const url = qs
    ? `/api/parsing/tasks?siteId=${siteId}&${qs}`
    : `/api/parsing/tasks?siteId=${siteId}`

  const response = await fetch(buildApiUrl(url), {
    method: "GET",
    headers: {
      "Accept": "application/json",
    },
  })

  return handleResponse<ParsingTaskListResponse>(response)
}

export async function fetchParsingTaskStatus(siteId: string) {
  const response = await fetch(
    buildApiUrl(`/api/parsing/status?siteId=${siteId}`),
    {
      method: "GET",
      headers: {
        "Accept": "application/json",
      },
    }
  )

  return handleResponse<TaskStatusResponse>(response)
}
