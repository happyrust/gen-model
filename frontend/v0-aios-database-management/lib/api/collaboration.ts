import type {
  CollaborationGroup,
  CreateCollaborationGroupPayload,
  UpdateCollaborationGroupPayload,
  RemoteSite,
  CreateRemoteSitePayload,
  SyncRecord,
  SyncOptions,
} from "@/types/collaboration"

import { getPublicApiBaseUrl } from "@/lib/env"

function buildApiUrl(path: string) {
  if (!path.startsWith("/")) {
    throw new Error(`API 路径必须以 / 开头: ${path}`)
  }
  const base = getPublicApiBaseUrl()
  if (!base) {
    return path
  }
  return `${base}${path}`
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

// ==================== 协同组管理 ====================

export async function fetchCollaborationGroups() {
  const response = await fetch(buildApiUrl("/api/collaboration-groups"), {
    method: "GET",
    headers: {
      Accept: "application/json",
    },
  })
  return handleResponse<{ items: CollaborationGroup[]; total: number }>(response)
}

export async function fetchCollaborationGroup(id: string) {
  const response = await fetch(buildApiUrl(`/api/collaboration-groups/${id}`), {
    method: "GET",
    headers: {
      Accept: "application/json",
    },
  })
  return handleResponse<CollaborationGroup>(response)
}

export async function createCollaborationGroup(payload: CreateCollaborationGroupPayload) {
  const response = await fetch(buildApiUrl("/api/collaboration-groups"), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(payload),
  })
  return handleResponse<{ status: string; item: CollaborationGroup }>(response)
}

export async function updateCollaborationGroup(id: string, payload: UpdateCollaborationGroupPayload) {
  const response = await fetch(buildApiUrl(`/api/collaboration-groups/${id}`), {
    method: "PUT",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(payload),
  })
  return handleResponse<{ status: string; item: CollaborationGroup }>(response)
}

export async function deleteCollaborationGroup(id: string) {
  const response = await fetch(buildApiUrl(`/api/collaboration-groups/${id}`), {
    method: "DELETE",
  })
  return handleResponse<{ status: string }>(response)
}

// ==================== 站点管理 ====================

export async function addSiteToGroup(groupId: string, siteId: string) {
  const response = await fetch(buildApiUrl(`/api/collaboration-groups/${groupId}/sites`), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ site_id: siteId }),
  })
  return handleResponse<{ status: string }>(response)
}

export async function removeSiteFromGroup(groupId: string, siteId: string) {
  const response = await fetch(buildApiUrl(`/api/collaboration-groups/${groupId}/sites/${siteId}`), {
    method: "DELETE",
  })
  return handleResponse<{ status: string }>(response)
}

export async function fetchGroupSites(groupId: string) {
  const response = await fetch(buildApiUrl(`/api/collaboration-groups/${groupId}/sites`), {
    method: "GET",
    headers: {
      Accept: "application/json",
    },
  })
  return handleResponse<{ items: any[] }>(response)
}

// ==================== 远程站点 ====================

export async function fetchRemoteSites() {
  const response = await fetch(buildApiUrl("/api/remote-sites"), {
    method: "GET",
    headers: {
      Accept: "application/json",
    },
  })
  return handleResponse<{ items: RemoteSite[] }>(response)
}

export async function createRemoteSite(payload: CreateRemoteSitePayload) {
  const response = await fetch(buildApiUrl("/api/remote-sites"), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(payload),
  })
  return handleResponse<{ status: string; item: RemoteSite }>(response)
}

export async function updateRemoteSite(id: string, payload: Partial<CreateRemoteSitePayload>) {
  const response = await fetch(buildApiUrl(`/api/remote-sites/${id}`), {
    method: "PUT",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(payload),
  })
  return handleResponse<{ status: string; item: RemoteSite }>(response)
}

export async function deleteRemoteSite(id: string) {
  const response = await fetch(buildApiUrl(`/api/remote-sites/${id}`), {
    method: "DELETE",
  })
  return handleResponse<{ status: string }>(response)
}

export async function testRemoteSiteConnection(id: string) {
  const response = await fetch(buildApiUrl(`/api/remote-sites/${id}/test`), {
    method: "POST",
  })
  return handleResponse<{
    status: string
    connection_status: string
    latency_ms?: number
    error?: string
  }>(response)
}

// ==================== 同步操作 ====================

export async function syncGroup(groupId: string, options?: SyncOptions) {
  const response = await fetch(buildApiUrl(`/api/collaboration-groups/${groupId}/sync`), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(options || {}),
  })
  return handleResponse<{ status: string; sync_id: string }>(response)
}

export async function pauseGroupSync(groupId: string) {
  const response = await fetch(buildApiUrl(`/api/collaboration-groups/${groupId}/pause`), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
  })
  return handleResponse<{ status: string }>(response)
}

export async function fetchSyncRecords(groupId: string) {
  const response = await fetch(buildApiUrl(`/api/collaboration-groups/${groupId}/sync-records`), {
    method: "GET",
    headers: {
      Accept: "application/json",
    },
  })
  return handleResponse<{ items: SyncRecord[] }>(response)
}

export async function fetchSyncStatus(groupId: string) {
  const response = await fetch(buildApiUrl(`/api/collaboration-groups/${groupId}/sync-status`), {
    method: "GET",
    headers: {
      Accept: "application/json",
    },
  })
  return handleResponse<{ status: string; current_sync?: any }>(response)
}

// ==================== 配置管理 ====================

export async function pushConfigToGroup(groupId: string) {
  const response = await fetch(buildApiUrl(`/api/collaboration-groups/${groupId}/push-config`), {
    method: "POST",
  })
  return handleResponse<{ status: string; affected_sites: number }>(response)
}

export async function fetchConfigDiff(groupId: string) {
  const response = await fetch(buildApiUrl(`/api/collaboration-groups/${groupId}/config-diff`), {
    method: "GET",
    headers: {
      Accept: "application/json",
    },
  })
  return handleResponse<{ diffs: any[] }>(response)
}

export async function resolveConfigConflict(groupId: string, resolution: any) {
  const response = await fetch(buildApiUrl(`/api/collaboration-groups/${groupId}/resolve-conflict`), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(resolution),
  })
  return handleResponse<{ status: string }>(response)
}
