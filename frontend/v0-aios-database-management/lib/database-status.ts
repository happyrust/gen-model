import { buildApiUrl, handleResponse } from "./api"

// ============ 数据库状态相关类型 ============

export interface DatabaseStatusResponse {
  success: boolean
  status: "Running" | "Starting" | "Stopped" | "Unknown"
  message?: string
  uptime?: number
  connections?: number
}

export interface StartDatabasePayload {
  ip: string
  port: number
  user: string
  password: string
  dbFile?: string
}

export interface StopDatabasePayload {
  ip: string
  port: number
}

// ============ 数据库状态相关 API ============

export async function fetchDatabaseStatus(ip: string, port: string | number) {
  const response = await fetch(
    buildApiUrl(`/api/database/startup/status?ip=${ip}&port=${port}`),
    {
      method: "GET",
      headers: {
        "Accept": "application/json",
      },
    }
  )

  return handleResponse<DatabaseStatusResponse>(response)
}

export async function startDatabase(payload: StartDatabasePayload) {
  const response = await fetch(buildApiUrl("/api/database/startup/start"), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(payload),
  })

  return handleResponse<DatabaseStatusResponse>(response)
}

export async function stopDatabase(ip: string, port: string | number) {
  const response = await fetch(buildApiUrl("/api/database/startup/stop"), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ ip, port }),
  })

  return handleResponse<DatabaseStatusResponse>(response)
}
