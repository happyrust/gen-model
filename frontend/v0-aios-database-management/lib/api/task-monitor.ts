import { buildApiUrl, handleResponse } from "../api"
import type { SystemMetrics } from "@/types/task-monitor"

interface RawTaskStatusResponse {
  tasks?: any[]
  total?: number
}

interface RawSystemStatus {
  cpu_usage?: number
  memory_usage?: number
  active_tasks?: number
  database_connected?: boolean
  surrealdb_connected?: boolean
  uptime?: { secs?: number; nanos?: number } | number | string
}

export async function fetchTaskStatus(): Promise<{
  tasks: any[]
  systemMetrics: SystemMetrics
  timestamp: string
}> {
  let rawTasks: any[] = []
  try {
    const response = await fetch(buildApiUrl("/api/tasks"), {
      method: "GET",
      cache: "no-store",
      headers: {
        Accept: "application/json",
      },
    })

    if (response.ok) {
      const data = (await response.json()) as RawTaskStatusResponse
      rawTasks = Array.isArray(data.tasks) ? data.tasks : []
    } else {
      console.warn("fetchTaskStatus: 获取任务失败", response.status)
    }
  } catch (error) {
    console.warn("fetchTaskStatus: 请求任务信息失败", error)
  }

  const systemMetrics = await fetchSystemMetrics()

  return {
    tasks: rawTasks,
    systemMetrics,
    timestamp: new Date().toISOString(),
  }
}

export async function startTask(taskId: string) {
  const response = await fetch(buildApiUrl(`/api/tasks/${taskId}/start`), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
  })

  return handleResponse<{ success: boolean; message?: string }>(response)
}

export async function stopTask(taskId: string) {
  const response = await fetch(buildApiUrl(`/api/tasks/${taskId}/stop`), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
  })

  return handleResponse<{ success: boolean; message?: string }>(response)
}

export async function pauseTask(taskId: string) {
  // 后端暂无单独暂停接口，复用 stop 行为
  return stopTask(taskId)
}

export async function resumeTask(taskId: string) {
  // 后端暂无单独恢复接口，复用 start 行为
  return startTask(taskId)
}

export async function cancelTask(taskId: string) {
  const response = await fetch(buildApiUrl(`/api/tasks/${taskId}`), {
    method: "DELETE",
    headers: {
      "Content-Type": "application/json",
    },
  })

  return handleResponse<{ success: boolean; message?: string }>(response)
}

export async function fetchTaskProgress(taskId: string) {
  const response = await fetch(buildApiUrl(`/api/tasks/${taskId}`), {
    method: "GET",
    headers: {
      Accept: "application/json",
    },
  })

  const data = await handleResponse<{
    progress?: { percentage?: number }
    status?: string
  }>(response)

  return {
    progress: data.progress?.percentage ?? 0,
    status: data.status ?? "unknown",
  }
}

export async function fetchSystemMetrics(): Promise<SystemMetrics> {
  try {
    const response = await fetch(buildApiUrl("/api/status"), {
      method: "GET",
      cache: "no-store",
      headers: {
        Accept: "application/json",
      },
    })

    if (!response.ok) {
      throw new Error(`系统状态接口返回 ${response.status}`)
    }

    const data = (await response.json()) as RawSystemStatus
    const uptimeSeconds = (() => {
      if (typeof data.uptime === "number") {
        return data.uptime
      }
      if (typeof data.uptime === "string") {
        const parsed = Number(data.uptime)
        return Number.isNaN(parsed) ? undefined : parsed
      }
      if (
        data.uptime &&
        typeof data.uptime === "object" &&
        ("secs" in data.uptime || "nanos" in data.uptime)
      ) {
        const secs = Number(
          (data.uptime as { secs?: number | string }).secs ?? 0
        )
        const nanos = Number(
          (data.uptime as { nanos?: number | string }).nanos ?? 0
        )
        return secs + nanos / 1_000_000_000
      }
      return undefined
    })()

    return {
      cpu: data.cpu_usage ?? 0,
      memory: data.memory_usage ?? 0,
      disk: 0,
      network: 0,
      uptimeSeconds,
      activeTasks: data.active_tasks,
      databaseConnected: data.database_connected,
      surrealdbConnected: data.surrealdb_connected,
    }
  } catch (error) {
    console.warn("fetchSystemMetrics: 请求系统状态失败", error)
    return {
      cpu: 0,
      memory: 0,
      disk: 0,
      network: 0,
    }
  }
}
