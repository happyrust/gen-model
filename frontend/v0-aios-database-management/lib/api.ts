const API_BASE_URL = process.env.NEXT_PUBLIC_API_BASE_URL ?? ""

/**
 * 构建完整的API URL
 * @param path API路径，必须以 / 开头
 * @returns 完整的API URL
 */
export function buildApiUrl(path: string): string {
  if (!path.startsWith("/")) {
    throw new Error(`API 路径必须以 / 开头: ${path}`)
  }
  if (!API_BASE_URL) {
    return path
  }
  return `${API_BASE_URL}${path}`
}

export async function handleResponse<T>(response: Response): Promise<T> {
  const text = await response.text()
  let data: unknown = null
  if (text) {
    try {
      data = JSON.parse(text)
    } catch (error) {
      throw new Error(`解析响应失败: ${String(error)}`)
    }
  }

  if (!response.ok) {
    const message =
      (typeof data === "object" && data && "error" in data && typeof (data as any).error === "string"
        ? (data as any).error
        : null) ||
      response.statusText ||
      "请求失败"
    throw new Error(message)
  }

  return data as T
}

export interface DeploymentSiteConfigPayload {
  name: string
  manual_db_nums: number[]
  project_name: string
  project_path: string
  project_code: number
  mdb_name: string
  module: string
  db_type: string
  surreal_ns: number
  db_ip: string
  db_port: string
  db_user: string
  db_password: string
  gen_model: boolean
  gen_mesh: boolean
  gen_spatial_tree: boolean
  apply_boolean_operation: boolean
  mesh_tol_ratio: number
  room_keyword: string
  target_sesno: number | null
}

export interface CreateDeploymentSitePayload {
  name: string
  description?: string
  root_directory?: string | null
  selected_projects: string[]
  config: DeploymentSiteConfigPayload
  env?: string | null
  owner?: string | null
  tags?: Record<string, unknown> | null
  notes?: string | null
}

export interface CreateDeploymentSiteResponse {
  status?: string
  item?: Record<string, any>
  message?: string
}

export interface DeploymentSiteListParams {
  q?: string
  status?: string
  env?: string
  owner?: string
  sort?: string
  page?: number
  per_page?: number
}

export interface DeploymentSiteListResponse {
  items: Array<Record<string, unknown>>
  total: number
  page: number
  per_page: number
  pages: number
}

export async function createDeploymentSite(payload: CreateDeploymentSitePayload) {
  const response = await fetch(buildApiUrl("/api/deployment-sites"), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(payload),
  })

  return handleResponse<CreateDeploymentSiteResponse>(response)
}

export async function fetchDeploymentSites(params: DeploymentSiteListParams) {
  const search = new URLSearchParams()
  if (params.q) search.set("q", params.q)
  if (params.status) search.set("status", params.status)
  if (params.env) search.set("env", params.env)
  if (params.owner) search.set("owner", params.owner)
  if (params.sort) search.set("sort", params.sort)
  if (typeof params.page === "number") search.set("page", String(params.page))
  if (typeof params.per_page === "number") search.set("per_page", String(params.per_page))

  const qs = search.toString()
  const url = qs ? `/api/deployment-sites?${qs}` : "/api/deployment-sites"
  const response = await fetch(buildApiUrl(url), {
    method: "GET",
    headers: {
      "Accept": "application/json",
    },
  })

  return handleResponse<DeploymentSiteListResponse>(response)
}

export async function patchDeploymentSite(siteId: string, payload: Record<string, unknown>) {
  const response = await fetch(buildApiUrl(`/api/deployment-sites/${siteId}`), {
    method: "PATCH",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(payload),
  })

  return handleResponse<{ status: string; item?: Record<string, unknown> }>(response)
}

export async function fetchDeploymentSite(siteId: string) {
  const response = await fetch(buildApiUrl(`/api/deployment-sites/${siteId}`), {
    method: "GET",
    headers: {
      "Accept": "application/json",
    },
  })

  return handleResponse<{ item: Record<string, unknown> }>(response)
}

export async function deleteDeploymentSite(siteId: string) {
  const response = await fetch(buildApiUrl(`/api/deployment-sites/${siteId}`), {
    method: "DELETE",
  })

  return handleResponse<{ status: string }>(response)
}
