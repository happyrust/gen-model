import {
  ApiErrorPayload,
  DatabaseConfig,
  DeploymentSiteDetailResponse,
  DeploymentSiteExportResponse,
  DeploymentSiteFilters,
  DeploymentSiteHealthResponse,
  DeploymentSiteImportRequest,
  DeploymentSiteImportResponse,
  DeploymentSiteTaskRequest,
  DeploymentSiteTaskResponse,
  DeploymentSitesResponse,
  DatabaseInfo,
  ModelGeneratePayload,
  ModelGenerateResponse,
  WizardTaskPayload,
  WizardTaskResponse,
  WizardTemplatesResponse,
} from "@/types/api"

const DEFAULT_BASE_URL = process.env.NEXT_PUBLIC_API_BASE_URL || "http://localhost:8010"
const sanitizedBaseUrl = DEFAULT_BASE_URL.endsWith("/")
  ? DEFAULT_BASE_URL.slice(0, -1)
  : DEFAULT_BASE_URL

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const url = path.startsWith("http")
    ? path
    : `${sanitizedBaseUrl}${path.startsWith("/") ? path : `/${path}`}`

  const method = (init.method || "GET").toUpperCase()
  const headers = new Headers(init.headers || {})
  if (!headers.has("Content-Type") && method !== "GET" && method !== "HEAD") {
    headers.set("Content-Type", "application/json")
  }

  const response = await fetch(url, {
    credentials: "include",
    ...init,
    headers,
  })

  if (!response.ok) {
    let message = `请求失败: ${response.status}`
    try {
      const payload = (await response.json()) as ApiErrorPayload
      if (payload?.message) {
        message = payload.message
      } else if (payload?.error) {
        message = payload.error
      }
    } catch {
      // ignore parse errors
    }
    throw new Error(message)
  }

  if (response.status === 204) {
    return {} as T
  }

  return (await response.json()) as T
}

export async function fetchDeploymentSites(params: DeploymentSiteFilters = {}) {
  const searchParams = new URLSearchParams()
  Object.entries(params).forEach(([key, value]) => {
    if (value === undefined || value === null || value === "") return
    searchParams.set(key, String(value))
  })

  const query = searchParams.toString()
  const path = query ? `/api/deployment-sites?${query}` : "/api/deployment-sites"
  return request<DeploymentSitesResponse>(path)
}

export async function fetchDeploymentSiteDetail(id: string) {
  return request<DeploymentSiteDetailResponse>(`/api/deployment-sites/${encodeURIComponent(id)}`)
}

export async function importDeploymentSite(payload: DeploymentSiteImportRequest) {
  return request<DeploymentSiteImportResponse>("/api/deployment-sites/import-dboption", {
    method: "POST",
    body: JSON.stringify(payload),
  })
}

export async function createDeploymentSiteTask(payload: DeploymentSiteTaskRequest) {
  return request<DeploymentSiteTaskResponse>(
    `/api/deployment-sites/${encodeURIComponent(payload.site_id)}/tasks`,
    {
      method: "POST",
      body: JSON.stringify(payload),
    },
  )
}

export async function healthcheckDeploymentSite(id: string) {
  return request<DeploymentSiteHealthResponse>(
    `/api/deployment-sites/${encodeURIComponent(id)}/healthcheck`,
    {
      method: "POST",
    },
  )
}

export async function exportDeploymentSiteConfig(id: string) {
  return request<DeploymentSiteExportResponse>(
    `/api/deployment-sites/${encodeURIComponent(id)}/export-config`,
  )
}

export async function fetchConfig() {
  return request<DatabaseConfig>("/api/config")
}

export async function fetchDatabases() {
  return request<DatabaseInfo[]>("/api/databases")
}

export async function generateObjModel(payload: ModelGeneratePayload) {
  return request<ModelGenerateResponse>("/api/model/generate-obj", {
    method: "POST",
    body: JSON.stringify(payload),
  })
}

export async function fetchWizardTemplates() {
  return request<WizardTemplatesResponse>("/api/wizard/templates")
}

export async function createWizardTask(payload: WizardTaskPayload) {
  return request<WizardTaskResponse>("/api/wizard/create-task", {
    method: "POST",
    body: JSON.stringify(payload),
  })
}

export const apiClient = {
  get: request,
}
