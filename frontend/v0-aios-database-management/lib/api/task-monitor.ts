import { buildApiUrl, handleResponse } from "../api"

export interface TaskStatusResponse {
  tasks: Task[]
  systemMetrics: SystemMetrics
  timestamp: string
}

export interface TaskActionResponse {
  success: boolean
  message: string
  taskId: string
}

// 获取任务状态
export async function fetchTaskStatus(): Promise<TaskStatusResponse> {
  const response = await fetch(buildApiUrl('/api/tasks/status'), {
    method: 'GET',
    headers: {
      'Accept': 'application/json',
    },
  })

  return handleResponse<TaskStatusResponse>(response)
}

// 启动任务
export async function startTask(taskId: string): Promise<TaskActionResponse> {
  const response = await fetch(buildApiUrl(`/api/tasks/${taskId}/start`), {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
  })

  return handleResponse<TaskActionResponse>(response)
}

// 停止任务
export async function stopTask(taskId: string): Promise<TaskActionResponse> {
  const response = await fetch(buildApiUrl(`/api/tasks/${taskId}/stop`), {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
  })

  return handleResponse<TaskActionResponse>(response)
}

// 暂停任务
export async function pauseTask(taskId: string): Promise<TaskActionResponse> {
  const response = await fetch(buildApiUrl(`/api/tasks/${taskId}/pause`), {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
  })

  return handleResponse<TaskActionResponse>(response)
}

// 恢复任务
export async function resumeTask(taskId: string): Promise<TaskActionResponse> {
  const response = await fetch(buildApiUrl(`/api/tasks/${taskId}/resume`), {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
  })

  return handleResponse<TaskActionResponse>(response)
}

// 取消任务
export async function cancelTask(taskId: string): Promise<TaskActionResponse> {
  const response = await fetch(buildApiUrl(`/api/tasks/${taskId}/cancel`), {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
  })

  return handleResponse<TaskActionResponse>(response)
}

// 获取任务进度
export async function fetchTaskProgress(taskId: string): Promise<{ progress: number; status: string }> {
  const response = await fetch(buildApiUrl(`/api/tasks/${taskId}/progress`), {
    method: 'GET',
    headers: {
      'Accept': 'application/json',
    },
  })

  return handleResponse<{ progress: number; status: string }>(response)
}

// 获取系统指标
export async function fetchSystemMetrics(): Promise<SystemMetrics> {
  const response = await fetch(buildApiUrl('/api/system/metrics'), {
    method: 'GET',
    headers: {
      'Accept': 'application/json',
    },
  })

  return handleResponse<SystemMetrics>(response)
}

// 导入类型定义
import type { Task, SystemMetrics } from "@/types/task-monitor"
