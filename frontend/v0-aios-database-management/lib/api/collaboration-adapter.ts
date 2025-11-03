import type { CollaborationGroup, RemoteSite, SyncRecord, SyncStrategy, CollaborationGroupStatus, ConnectionStatus } from "@/types/collaboration"

interface NodeStatus {
  is_primary: boolean
  primary_url: string | null
  node_name: string | null
}

let cachedNodeStatus: NodeStatus | null = null
let nodeStatusFetchTime: number = 0
const NODE_STATUS_CACHE_TTL = 30000

export interface RemoteSyncEnv {
  id: string
  name: string
  mqtt_host: string | null
  mqtt_port: number | null
  mqtt_user: string | null
  mqtt_password: string | null
  file_server_host: string | null
  location: string | null
  location_dbs: string | null
  reconnect_initial_ms: number | null
  reconnect_max_ms: number | null
  status: string | null
  created_at: string
  updated_at: string | null
}

export interface RemoteSyncSite {
  id: string
  env_id: string
  site_name: string
  site_description: string | null
  site_host: string | null
  site_location: string | null
  site_location_dbs: string | null
  topics_subscribe: string | null
  topics_publish: string | null
  is_local: boolean
  status: string | null
  last_sync_at: string | null
  created_at: string
  updated_at: string | null
}

export interface RemoteSyncEnvCreatePayload {
  name: string
  mqtt_host: string
  mqtt_port: number
  mqtt_user?: string
  mqtt_password?: string
  file_server_host?: string
  location?: string
  location_dbs?: string
  reconnect_initial_ms?: number
  reconnect_max_ms?: number
}

export interface RemoteSyncSiteCreatePayload {
  site_name: string
  site_description?: string
  site_host?: string
  site_location?: string
  site_location_dbs?: string
  topics_subscribe?: string[]
  topics_publish?: string[]
  is_local: boolean
}

const API_BASE_URL = process.env.NEXT_PUBLIC_API_BASE_URL ?? ""

function buildApiUrl(path: string) {
  if (!path.startsWith("/")) {
    throw new Error(`API 路径必须以 / 开头: ${path}`)
  }
  if (!API_BASE_URL) {
    return path
  }
  return `${API_BASE_URL}${path}`
}

async function getNodeStatus(): Promise<NodeStatus | null> {
  const now = Date.now()
  if (cachedNodeStatus && (now - nodeStatusFetchTime) < NODE_STATUS_CACHE_TTL) {
    return cachedNodeStatus
  }

  try {
    const response = await fetch(buildApiUrl("/api/node-status"))
    if (!response.ok) return null
    const data = await response.json()
    cachedNodeStatus = data.node
    nodeStatusFetchTime = now
    return cachedNodeStatus
  } catch (error) {
    console.error("Failed to fetch node status:", error)
    return null
  }
}

async function ensurePrimaryNode(): Promise<string | null> {
  const status = await getNodeStatus()
  if (!status) return null

  if (!status.is_primary && status.primary_url) {
    return status.primary_url
  }
  return null
}

async function handleResponse<T>(response: Response): Promise<T> {
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

export async function listRemoteSyncEnvs(): Promise<RemoteSyncEnv[]> {
  const response = await fetch(buildApiUrl("/api/remote-sync/envs"), {
    method: "GET",
    headers: { "Accept": "application/json" },
  })
  return handleResponse<RemoteSyncEnv[]>(response)
}

export async function getRemoteSyncEnv(id: string): Promise<RemoteSyncEnv> {
  const response = await fetch(buildApiUrl(`/api/remote-sync/envs/${id}`), {
    method: "GET",
    headers: { "Accept": "application/json" },
  })
  return handleResponse<RemoteSyncEnv>(response)
}

export async function createRemoteSyncEnv(payload: RemoteSyncEnvCreatePayload): Promise<RemoteSyncEnv> {
  const primaryUrl = await ensurePrimaryNode()
  if (primaryUrl) {
    const redirectUrl = `${primaryUrl}/api/remote-sync/envs`
    const response = await fetch(redirectUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    })
    return handleResponse<RemoteSyncEnv>(response)
  }

  const response = await fetch(buildApiUrl("/api/remote-sync/envs"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  })
  return handleResponse<RemoteSyncEnv>(response)
}

export async function updateRemoteSyncEnv(id: string, payload: Partial<RemoteSyncEnvCreatePayload>): Promise<RemoteSyncEnv> {
  const primaryUrl = await ensurePrimaryNode()
  if (primaryUrl) {
    const redirectUrl = `${primaryUrl}/api/remote-sync/envs/${id}`
    const response = await fetch(redirectUrl, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    })
    return handleResponse<RemoteSyncEnv>(response)
  }

  const response = await fetch(buildApiUrl(`/api/remote-sync/envs/${id}`), {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  })
  return handleResponse<RemoteSyncEnv>(response)
}

export async function deleteRemoteSyncEnv(id: string): Promise<void> {
  const primaryUrl = await ensurePrimaryNode()
  if (primaryUrl) {
    const redirectUrl = `${primaryUrl}/api/remote-sync/envs/${id}`
    const response = await fetch(redirectUrl, {
      method: "DELETE",
    })
    await handleResponse<void>(response)
    return
  }

  const response = await fetch(buildApiUrl(`/api/remote-sync/envs/${id}`), {
    method: "DELETE",
  })
  await handleResponse<void>(response)
}

export async function activateRemoteSyncEnv(id: string): Promise<{ message: string }> {
  const response = await fetch(buildApiUrl(`/api/remote-sync/envs/${id}/activate`), {
    method: "POST",
  })
  return handleResponse<{ message: string }>(response)
}

export async function stopRemoteSyncEnv(id: string): Promise<{ message: string }> {
  const response = await fetch(buildApiUrl(`/api/remote-sync/envs/${id}/stop`), {
    method: "POST",
  })
  return handleResponse<{ message: string }>(response)
}

export async function listRemoteSyncSites(envId: string): Promise<RemoteSyncSite[]> {
  const response = await fetch(buildApiUrl(`/api/remote-sync/envs/${envId}/sites`), {
    method: "GET",
    headers: { "Accept": "application/json" },
  })
  return handleResponse<RemoteSyncSite[]>(response)
}

export async function createRemoteSyncSite(envId: string, payload: RemoteSyncSiteCreatePayload): Promise<RemoteSyncSite> {
  const response = await fetch(buildApiUrl(`/api/remote-sync/envs/${envId}/sites`), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  })
  return handleResponse<RemoteSyncSite>(response)
}

export async function updateRemoteSyncSite(envId: string, siteId: string, payload: Partial<RemoteSyncSiteCreatePayload>): Promise<RemoteSyncSite> {
  const response = await fetch(buildApiUrl(`/api/remote-sync/envs/${envId}/sites/${siteId}`), {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  })
  return handleResponse<RemoteSyncSite>(response)
}

export async function deleteRemoteSyncSite(envId: string, siteId: string): Promise<void> {
  const response = await fetch(buildApiUrl(`/api/remote-sync/envs/${envId}/sites/${siteId}`), {
    method: "DELETE",
  })
  await handleResponse<void>(response)
}

export function envToGroup(env: RemoteSyncEnv): CollaborationGroup {
  const status: CollaborationGroupStatus =
    env.status === "active" ? "Active" :
    env.status === "inactive" ? "Inactive" :
    env.status === "error" ? "Error" : "Pending"

  const syncStrategy: SyncStrategy = {
    mode: "OneWay",
    interval_seconds: 60,
    auto_sync: true,
    conflict_resolution: "LatestWins",
  }

  return {
    id: env.id,
    name: env.name,
    group_type: "DataSync",
    site_ids: [],
    sync_strategy: syncStrategy,
    status,
    created_at: env.created_at,
    updated_at: env.updated_at || env.created_at,
    shared_config: {
      mqtt_broker: env.mqtt_host || "",
      mqtt_port: env.mqtt_port || 1883,
      mqtt_username: env.mqtt_user,
      mqtt_password: env.mqtt_password,
      file_server_url: env.file_server_host,
    },
    location: env.location || undefined,
    creator: "system",
  }
}

export function groupToEnvPayload(group: Partial<CollaborationGroup>): RemoteSyncEnvCreatePayload {
  return {
    name: group.name || "",
    mqtt_host: group.shared_config?.mqtt_broker || "",
    mqtt_port: group.shared_config?.mqtt_port || 1883,
    mqtt_user: group.shared_config?.mqtt_username,
    mqtt_password: group.shared_config?.mqtt_password,
    file_server_host: group.shared_config?.file_server_url,
    location: group.location,
    location_dbs: undefined,
    reconnect_initial_ms: 1000,
    reconnect_max_ms: 60000,
  }
}

export function siteToRemoteSite(site: RemoteSyncSite): RemoteSite {
  const mapStatus = (status: string): ConnectionStatus => {
    if (status === "active") return "Online"
    if (status === "error") return "Failed"
    return "Offline"
  }

  return {
    id: site.id,
    name: site.site_name,
    location: site.site_location || "",
    ip_address: site.site_host || "",
    status: mapStatus(site.status || ""),
    last_sync: site.last_sync_at || undefined,
    data_version: undefined,
    is_local: site.is_local,
  }
}

export function remoteSiteToSitePayload(site: Partial<RemoteSite>): RemoteSyncSiteCreatePayload {
  return {
    site_name: site.name || "",
    site_description: undefined,
    site_host: site.ip_address,
    site_location: site.location,
    site_location_dbs: undefined,
    topics_subscribe: undefined,
    topics_publish: undefined,
    is_local: site.is_local || false,
  }
}