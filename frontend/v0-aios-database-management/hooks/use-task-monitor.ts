"use client"

import { useState, useEffect, useCallback } from "react"
import { fetchTaskStatus, startTask, stopTask, pauseTask } from "@/lib/api/task-monitor"
import type { Task, SystemMetrics, TaskStatus } from "@/types/task-monitor"

function normalizeStatus(value: unknown): TaskStatus {
  if (typeof value === "string") {
    const lower = value.toLowerCase()
    if (
      lower === "pending" ||
      lower === "running" ||
      lower === "completed" ||
      lower === "failed" ||
      lower === "cancelled"
    ) {
      return lower
    }
    if (lower === "canceled") {
      return "cancelled"
    }
  }
  return "unknown"
}

function normalizeType(value: unknown): string {
  if (typeof value === "string") {
    return value
  }
  if (value && typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>)
    if (entries.length > 0) {
      const [key, val] = entries[0]
      if (key === "Custom" && typeof val === "string") {
        return val
      }
      return key
    }
  }
  return "Unknown"
}

function toIsoTimestamp(value: unknown): string | undefined {
  if (typeof value === "number") {
    return new Date(value).toISOString()
  }
  if (typeof value === "string") {
    if (/^\d+$/.test(value)) {
      const asNumber = Number(value)
      if (!Number.isNaN(asNumber)) {
        return new Date(asNumber).toISOString()
      }
    }
    return value
  }
  if (
    value &&
    typeof value === "object" &&
    "secs" in (value as Record<string, unknown>)
  ) {
    const secs = Number(
      (value as { secs?: number | string }).secs ?? 0
    )
    const nanos = Number(
      (value as { nanos?: number | string }).nanos ?? 0
    )
    const millis = secs * 1000 + Math.floor(nanos / 1_000_000)
    return new Date(millis).toISOString()
  }
  return undefined
}

function mapTask(raw: any): Task {
  const status = normalizeStatus(raw?.status)
  const startTime = toIsoTimestamp(raw?.started_at)
  const endTime = toIsoTimestamp(raw?.completed_at)
  const durationMs =
    typeof raw?.actual_duration === "number"
      ? raw.actual_duration
      : startTime && endTime
        ? Math.max(
            0,
            new Date(endTime).getTime() -
              new Date(startTime).getTime()
          )
        : undefined

  const progressPercentage =
    typeof raw?.progress?.percentage === "number"
      ? raw.progress.percentage
      : 0

  return {
    id: String(raw?.id ?? ""),
    name: String(raw?.name ?? "未命名任务"),
    type: normalizeType(raw?.task_type),
    status,
    progress: progressPercentage,
    startTime,
    endTime,
    durationMs,
    estimatedTime:
      typeof raw?.estimated_duration === "number"
        ? raw.estimated_duration
        : undefined,
    priority: raw?.priority,
    parameters: raw?.config ?? {},
    result: {
      success: status === "completed",
      message:
        typeof raw?.error === "string"
          ? raw.error
          : raw?.error_details?.detailed_message,
    },
    error: typeof raw?.error === "string" ? raw.error : undefined,
    raw,
  }
}

interface TaskMonitorState {
  tasks: Task[]
  systemMetrics: SystemMetrics
  isConnected: boolean
  lastUpdate: string
  error: string | null
}

export function useTaskMonitor() {
  const [state, setState] = useState<TaskMonitorState>({
    tasks: [],
    systemMetrics: {
      cpu: 0,
      memory: 0,
    },
    isConnected: false,
    lastUpdate: new Date().toISOString(),
    error: null
  })

  const [loading, setLoading] = useState(false)

  // 刷新数据
  const refreshData = useCallback(async () => {
    setLoading(true)
    setState(prev => ({ ...prev, error: null }))

    try {
      const response = await fetchTaskStatus()
      
      setState(prev => ({
        ...prev,
        tasks: Array.isArray(response.tasks)
          ? response.tasks.map(mapTask)
          : [],
        systemMetrics: response.systemMetrics || prev.systemMetrics,
        lastUpdate: new Date().toISOString(),
        isConnected: true
      }))
    } catch (error) {
      console.error('Failed to fetch task status:', error)
      setState(prev => ({
        ...prev,
        error: error instanceof Error ? error.message : '获取任务状态失败',
        isConnected: false
      }))
    } finally {
      setLoading(false)
    }
  }, [])

  // 启动任务
  const handleStartTask = useCallback(async (taskId: string) => {
    try {
      await startTask(taskId)
      await refreshData() // 刷新数据
    } catch (error) {
      console.error('Failed to start task:', error)
      setState(prev => ({
        ...prev,
        error: error instanceof Error ? error.message : '启动任务失败'
      }))
    }
  }, [refreshData])

  // 停止任务
  const handleStopTask = useCallback(async (taskId: string) => {
    try {
      await stopTask(taskId)
      await refreshData() // 刷新数据
    } catch (error) {
      console.error('Failed to stop task:', error)
      setState(prev => ({
        ...prev,
        error: error instanceof Error ? error.message : '停止任务失败'
      }))
    }
  }, [refreshData])

  // 暂停任务
  const handlePauseTask = useCallback(async (taskId: string) => {
    try {
      await pauseTask(taskId)
      await refreshData() // 刷新数据
    } catch (error) {
      console.error('Failed to pause task:', error)
      setState(prev => ({
        ...prev,
        error: error instanceof Error ? error.message : '暂停任务失败'
      }))
    }
  }, [refreshData])

  // 初始化加载
  useEffect(() => {
    refreshData()
  }, [refreshData])

  return {
    ...state,
    loading,
    refreshData,
    startTask: handleStartTask,
    stopTask: handleStopTask,
    pauseTask: handlePauseTask
  }
}
