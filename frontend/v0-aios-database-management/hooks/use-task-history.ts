"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import type {
  HistoryFilters,
  LogEntry,
  LogLevel,
  PaginationState,
  TaskHistory,
  TaskReplayResponse,
  TaskStatus,
} from "@/types/task-history"

const DEFAULT_FILTERS: HistoryFilters = {
  status: "all",
  type: "all",
  search: "",
  dateRange: null,
  sortBy: "startTime",
  sortOrder: "desc",
}

const DEFAULT_PAGINATION: PaginationState = {
  currentPage: 1,
  pageSize: 20,
  totalPages: 1,
  totalItems: 0,
}

const MAX_FETCH_LIMIT = 500

function normalizeStatus(value: unknown): TaskStatus {
  if (typeof value === "string") {
    const normalized = value.toLowerCase()
    switch (normalized) {
      case "pending":
      case "running":
      case "completed":
      case "failed":
      case "cancelled":
        return normalized
      case "canceled":
        return "cancelled"
      default:
        break
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

function mapLogLevel(value: unknown): LogLevel {
  if (typeof value === "string") {
    const lower = value.toLowerCase()
    if (lower === "warning") return "warning"
    if (lower === "warn") return "warn"
    if (lower === "critical") return "critical"
    if (lower === "error") return "error"
    if (lower === "debug") return "debug"
  }
  return "info"
}

function mapLogEntry(taskId: string, raw: any, index: number): LogEntry {
  const timestamp =
    toIsoTimestamp(raw?.timestamp) ?? new Date().toISOString()
  return {
    id: raw?.id
      ? String(raw.id)
      : `${taskId}-log-${timestamp}-${index}`,
    taskId,
    level: mapLogLevel(raw?.level),
    message:
      typeof raw?.message === "string"
        ? raw.message
        : JSON.stringify(raw?.message ?? ""),
    timestamp,
    source:
      typeof raw?.source === "string"
        ? raw.source
        : raw?.error_code
          ? String(raw.error_code)
          : undefined,
    metadata: raw ?? undefined,
  }
}

function computeDurationMs(
  start?: string,
  end?: string,
  fallback?: number
): number | undefined {
  if (typeof fallback === "number" && !Number.isNaN(fallback)) {
    return fallback
  }
  if (start && end) {
    const startMs = new Date(start).getTime()
    const endMs = new Date(end).getTime()
    if (!Number.isNaN(startMs) && !Number.isNaN(endMs)) {
      return Math.max(0, endMs - startMs)
    }
  }
  return undefined
}

function mapTaskInfo(raw: any): TaskHistory {
  const id = String(raw?.id ?? "")
  const status = normalizeStatus(raw?.status)
  const createdAt = toIsoTimestamp(raw?.created_at)
  const startTime = toIsoTimestamp(raw?.started_at)
  const endTime = toIsoTimestamp(raw?.completed_at)
  const durationMs = computeDurationMs(
    startTime,
    endTime,
    typeof raw?.actual_duration === "number"
      ? raw.actual_duration
      : undefined
  )
  const logsArray = Array.isArray(raw?.logs) ? raw.logs : []
  const logs = logsArray.map((log: any, index: number) =>
    mapLogEntry(id, log, index)
  )

  return {
    id,
    taskId: id,
    name: String(raw?.name ?? "未命名任务"),
    type: normalizeType(raw?.task_type),
    status,
    startTime,
    endTime,
    durationMs,
    result: {
      success: status === "completed",
      message:
        typeof raw?.error === "string"
          ? raw.error
          : raw?.error_details?.detailed_message,
    },
    parameters: raw?.config ?? {},
    logs,
    createdAt,
    raw,
  }
}

async function fetchTasksFromBackend(
  filters: HistoryFilters,
  limit: number
): Promise<any[]> {
  const params = new URLSearchParams()
  const statusFilter =
    filters.status !== "all" ? filters.status.toLowerCase() : undefined

  if (statusFilter) {
    params.set("status", statusFilter)
  }
  params.set("limit", String(Math.min(MAX_FETCH_LIMIT, Math.max(limit, 1))))

  const response = await fetch(`/api/tasks?${params.toString()}`, {
    cache: "no-store",
  })

  if (!response.ok) {
    throw new Error(`获取任务失败: ${response.status}`)
  }

  const data = await response.json()
  return Array.isArray(data?.tasks) ? data.tasks : []
}

async function replayTask(
  taskId: string,
  parameters?: Record<string, any>
): Promise<TaskReplayResponse> {
  try {
    const response = await fetch(`/api/tasks/${taskId}/restart`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ parameters }),
    })

    if (!response.ok) {
      throw new Error(`任务重启失败: ${response.status}`)
    }

    return (await response.json()) as TaskReplayResponse
  } catch (error) {
    console.warn("任务重启接口不可用:", error)
    return {
      success: false,
      message: "任务重启接口不可用",
    }
  }
}

export function useTaskHistory() {
  const [tasks, setTasks] = useState<TaskHistory[]>([])
  const [allTasks, setAllTasks] = useState<TaskHistory[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [filters, setFiltersState] =
    useState<HistoryFilters>(DEFAULT_FILTERS)
  const [pagination, setPaginationState] = useState<PaginationState>(
    DEFAULT_PAGINATION
  )

  const loadHistory = useCallback(async () => {
    setLoading(true)
    setError(null)

    try {
      const fetchLimit =
        pagination.pageSize *
        Math.max(1, pagination.currentPage) *
        2
      const rawTasks = await fetchTasksFromBackend(
        filters,
        fetchLimit
      )
      const mappedTasks = rawTasks.map(mapTaskInfo)

      const filteredTasks = mappedTasks.filter((task) => {
        if (
          filters.type !== "all" &&
          task.type !== filters.type
        ) {
          return false
        }
        if (filters.search.trim()) {
          const keyword = filters.search.trim().toLowerCase()
          const nameIncludes = task.name
            .toLowerCase()
            .includes(keyword)
          const idIncludes = task.taskId
            .toLowerCase()
            .includes(keyword)
          if (!nameIncludes && !idIncludes) {
            return false
          }
        }
        if (filters.dateRange) {
          const [start, end] = filters.dateRange
          const baseTime =
            task.startTime ??
            task.createdAt ??
            task.endTime
          if (!baseTime) {
            return false
          }
          const ts = new Date(baseTime)
          if (Number.isNaN(ts.getTime())) {
            return false
          }
          if (ts < start || ts > end) {
            return false
          }
        }
        return true
      })

      const sortedTasks = filteredTasks.sort((a, b) => {
        const order = filters.sortOrder === "asc" ? 1 : -1
        const compareValues = (
          valueA: number | string | undefined,
          valueB: number | string | undefined
        ) => {
          if (valueA === undefined && valueB === undefined) return 0
          if (valueA === undefined) return -1
          if (valueB === undefined) return 1
          if (typeof valueA === "number" && typeof valueB === "number") {
            return valueA - valueB
          }
          return String(valueA).localeCompare(String(valueB))
        }

        switch (filters.sortBy) {
          case "endTime":
            return (
              compareValues(
                new Date(a.endTime ?? 0).getTime(),
                new Date(b.endTime ?? 0).getTime()
              ) * order
            )
          case "duration":
            return (
              compareValues(a.durationMs ?? 0, b.durationMs ?? 0) *
              order
            )
          case "status":
            return (
              compareValues(a.status, b.status) *
              order
            )
          case "startTime":
          default:
            return (
              compareValues(
                new Date(a.startTime ?? a.createdAt ?? 0).getTime(),
                new Date(b.startTime ?? b.createdAt ?? 0).getTime()
              ) * order
            )
        }
      })

      const totalItems = sortedTasks.length
      const pageSize = pagination.pageSize
      const totalPages = Math.max(1, Math.ceil(totalItems / pageSize))
      const desiredPage = Math.min(
        pagination.currentPage,
        totalPages
      )
      const startIndex = (desiredPage - 1) * pageSize
      const paginatedTasks = sortedTasks.slice(
        startIndex,
        startIndex + pageSize
      )

      setAllTasks(sortedTasks)
      setTasks(paginatedTasks)
      setPaginationState((prev) => {
        if (
          prev.currentPage === desiredPage &&
          prev.totalItems === totalItems &&
          prev.totalPages === totalPages
        ) {
          return prev
        }
        return {
          ...prev,
          currentPage: desiredPage,
          totalItems,
          totalPages,
        }
      })
    } catch (err) {
      console.error("加载任务历史失败:", err)
      setError(
        err instanceof Error ? err.message : "加载任务历史失败"
      )
    } finally {
      setLoading(false)
    }
  }, [filters, pagination.currentPage, pagination.pageSize])

  const refreshHistory = useCallback(async () => {
    await loadHistory()
  }, [loadHistory])

  const updateFilters = useCallback(
    (updates: Partial<HistoryFilters>) => {
      setFiltersState((prev) => ({
        ...prev,
        ...updates,
      }))
      setPaginationState((prev) => ({
        ...prev,
        currentPage: 1,
      }))
    },
    []
  )

  const updatePagination = useCallback(
    (updates: Partial<PaginationState>) => {
      setPaginationState((prev) => ({
        ...prev,
        ...updates,
      }))
    },
    []
  )

  const replayTaskHistory = useCallback(
    async (taskId: string, parameters?: Record<string, any>) => {
      const response = await replayTask(taskId, parameters)
      if (response.success) {
        await refreshHistory()
      }
      return response
    },
    [refreshHistory]
  )

  const getTaskDetails = useCallback(
    (taskId: string) => allTasks.find((task) => task.id === taskId),
    [allTasks]
  )

  const statistics = useMemo(() => {
    const total = allTasks.length
    const completed = allTasks.filter(
      (task) => task.status === "completed"
    ).length
    const failed = allTasks.filter(
      (task) => task.status === "failed"
    ).length
    const cancelled = allTasks.filter(
      (task) => task.status === "cancelled"
    ).length
    const running = allTasks.filter(
      (task) => task.status === "running"
    ).length
    const pending = allTasks.filter(
      (task) => task.status === "pending"
    ).length

    const avgDuration =
      total > 0
        ? allTasks.reduce(
            (sum, task) => sum + (task.durationMs ?? 0),
            0
          ) / total
        : 0

    return {
      total,
      completed,
      failed,
      cancelled,
      running,
      pending,
      successRate: total > 0 ? (completed / total) * 100 : 0,
      failureRate: total > 0 ? (failed / total) * 100 : 0,
      avgDuration,
    }
  }, [allTasks])

  const statisticsByType = useMemo(() => {
    const grouped = new Map<
      string,
      { total: number; completed: number; failed: number; durationSum: number }
    >()

    allTasks.forEach((task) => {
      const entry =
        grouped.get(task.type) ??
        { total: 0, completed: 0, failed: 0, durationSum: 0 }
      entry.total += 1
      if (task.status === "completed") entry.completed += 1
      if (task.status === "failed") entry.failed += 1
      entry.durationSum += task.durationMs ?? 0
      grouped.set(task.type, entry)
    })

    return Array.from(grouped.entries()).map(([type, values]) => ({
      type,
      total: values.total,
      completed: values.completed,
      failed: values.failed,
      avgDuration:
        values.total > 0 ? values.durationSum / values.total : 0,
    }))
  }, [allTasks])

  const statisticsByDate = useCallback(
    (days: number = 7) => {
      const now = new Date()
      const startBoundary = new Date(
        now.getTime() - days * 24 * 60 * 60 * 1000
      )

      const buckets = new Map<string, TaskHistory[]>()
      allTasks.forEach((task) => {
        const dateSource = task.startTime ?? task.createdAt
        if (!dateSource) {
          return
        }
        const date = new Date(dateSource)
        if (Number.isNaN(date.getTime())) {
          return
        }
        if (date < startBoundary) {
          return
        }
        const key = date.toISOString().split("T")[0]
        const existing = buckets.get(key)
        if (existing) {
          existing.push(task)
        } else {
          buckets.set(key, [task])
        }
      })

      return Array.from(buckets.entries())
        .map(([date, entries]) => ({
          date,
          total: entries.length,
          completed: entries.filter(
            (task) => task.status === "completed"
          ).length,
          failed: entries.filter(
            (task) => task.status === "failed"
          ).length,
        }))
        .sort((a, b) => a.date.localeCompare(b.date))
    },
    [allTasks]
  )

  useEffect(() => {
    void loadHistory()
  }, [loadHistory])

  return {
    tasks,
    allTasks,
    loading,
    error,
    filters,
    pagination,
    loadHistory,
    refreshHistory,
    setFilters: updateFilters,
    setPagination: updatePagination,
    replayTask: replayTaskHistory,
    getTaskDetails,
    getTaskStatistics: () => statistics,
    getStatisticsByType: () => statisticsByType,
    getStatisticsByDate: statisticsByDate,
  }
}

