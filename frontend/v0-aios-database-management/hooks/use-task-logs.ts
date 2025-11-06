"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import type {
  LogEntry,
  LogFilters,
  LogLevel,
  PaginationState,
} from "@/types/task-logs"

const DEFAULT_FILTERS: LogFilters = {
  level: "all",
  search: "",
  dateRange: null,
  taskId: "all",
}

const DEFAULT_PAGINATION: PaginationState = {
  currentPage: 1,
  pageSize: 100,
  totalPages: 1,
  totalItems: 0,
}

const MAX_TASKS_FOR_GLOBAL_LOGS = 200

function mapBackendLogLevel(value: LogLevel | "all"): string | undefined {
  if (value === "all") return undefined
  switch (value) {
    case "info":
      return "Info"
    case "warn":
    case "warning":
      return "Warning"
    case "error":
      return "Error"
    case "critical":
      return "Critical"
    case "debug":
      return "Debug"
    default:
      return undefined
  }
}

function normalizeLogLevel(value: unknown): LogLevel {
  if (typeof value === "string") {
    const lower = value.toLowerCase()
    if (
      lower === "info" ||
      lower === "warn" ||
      lower === "warning" ||
      lower === "error" ||
      lower === "debug" ||
      lower === "critical"
    ) {
      return lower as LogLevel
    }
  }
  return "info"
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
    const secs = Number((value as { secs?: number | string }).secs ?? 0)
    const nanos = Number(
      (value as { nanos?: number | string }).nanos ?? 0
    )
    const millis = secs * 1000 + Math.floor(nanos / 1_000_000)
    return new Date(millis).toISOString()
  }
  return undefined
}

function mapLogEntry(taskId: string, raw: any, index: number): LogEntry {
  const timestamp =
    toIsoTimestamp(raw?.timestamp) ?? new Date().toISOString()
  return {
    id: raw?.id
      ? String(raw.id)
      : `${taskId}-log-${timestamp}-${index}`,
    taskId,
    level: normalizeLogLevel(raw?.level),
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
    stackTrace:
      typeof raw?.stack_trace === "string"
        ? raw.stack_trace
        : undefined,
  }
}

async function fetchTaskLogsFromBackend(
  taskId: string,
  filters: LogFilters,
  pagination: PaginationState
): Promise<{
  logs: LogEntry[]
  total: number
}> {
  const params = new URLSearchParams()
  params.set("limit", String(pagination.pageSize))
  params.set(
    "offset",
    String((pagination.currentPage - 1) * pagination.pageSize)
  )

  const backendLevel = mapBackendLogLevel(filters.level)
  if (backendLevel) {
    params.set("level", backendLevel)
  }
  if (filters.search.trim()) {
    params.set("search", filters.search.trim())
  }

  const response = await fetch(
    `/api/tasks/${encodeURIComponent(taskId)}/logs?${params.toString()}`,
    { cache: "no-store" }
  )

  if (!response.ok) {
    throw new Error(`获取任务日志失败: ${response.status}`)
  }

  const data = await response.json()
  const logsArray = Array.isArray(data?.logs) ? data.logs : []
  const mappedLogs = logsArray.map((log: any, index: number) =>
    mapLogEntry(taskId, log, index)
  )

  const dateFilteredLogs = filters.dateRange
    ? mappedLogs.filter((log: LogEntry) => {
        const ts = new Date(log.timestamp)
        const [start, end] = filters.dateRange as [Date, Date]
        return ts >= start && ts <= end
      })
    : mappedLogs

  const total =
    filters.dateRange || filters.search.trim()
      ? dateFilteredLogs.length
      : Number(data?.total_count) || mappedLogs.length

  return {
    logs: dateFilteredLogs,
    total,
  }
}

async function fetchAllTaskLogs(
  filters: LogFilters
): Promise<LogEntry[]> {
  const response = await fetch(`/api/tasks?limit=${MAX_TASKS_FOR_GLOBAL_LOGS}`, {
    cache: "no-store",
  })

  if (!response.ok) {
    throw new Error(`获取任务列表失败: ${response.status}`)
  }

  const data = await response.json()
  const tasks = Array.isArray(data?.tasks) ? data.tasks : []

  const aggregated: LogEntry[] = []
  tasks.forEach((task: any, taskIndex: number) => {
    const taskId = String(task?.id ?? `task-${taskIndex}`)
    const logsArray = Array.isArray(task?.logs) ? task.logs : []
    logsArray.forEach((log: any, logIndex: number) => {
      aggregated.push(mapLogEntry(taskId, log, logIndex))
    })
  })

  return aggregated
}

export function useTaskLogs(taskId?: string) {
  const [logs, setLogs] = useState<LogEntry[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [filters, setFiltersState] =
    useState<LogFilters>({
      ...DEFAULT_FILTERS,
      taskId: taskId ?? "all",
    })
  const [pagination, setPaginationState] = useState<PaginationState>(
    DEFAULT_PAGINATION
  )

  const loadLogs = useCallback(async () => {
    setLoading(true)
    setError(null)

    try {
      if (!filters.taskId || filters.taskId === "all") {
        const allLogs = await fetchAllTaskLogs(filters)

        const levelFiltered =
          filters.level === "all"
            ? allLogs
            : allLogs.filter(
                (log) => log.level === normalizeLogLevel(filters.level)
              )

        const searchFiltered = filters.search.trim()
          ? levelFiltered.filter((log) =>
              log.message
                .toLowerCase()
                .includes(filters.search.trim().toLowerCase())
            )
          : levelFiltered

        const dateFiltered = filters.dateRange
          ? searchFiltered.filter((log) => {
              const ts = new Date(log.timestamp)
              const [start, end] = filters.dateRange as [Date, Date]
              return ts >= start && ts <= end
            })
          : searchFiltered

        const totalItems = dateFiltered.length
        const totalPages = Math.max(
          1,
          Math.ceil(totalItems / pagination.pageSize)
        )
        const safePage = Math.min(pagination.currentPage, totalPages)
        const startIndex =
          (safePage - 1) * pagination.pageSize
        const pageItems = dateFiltered.slice(
          startIndex,
          startIndex + pagination.pageSize
        )

        setLogs(pageItems)
        setPaginationState((prev) => ({
          ...prev,
          currentPage: safePage,
          totalItems,
          totalPages,
        }))
      } else {
        const { logs: taskLogs, total } =
          await fetchTaskLogsFromBackend(
            filters.taskId,
            filters,
            pagination
          )

        setLogs(taskLogs)
        const totalPages = Math.max(
          1,
          Math.ceil(total / pagination.pageSize)
        )
        setPaginationState((prev) => ({
          ...prev,
          totalItems: total,
          totalPages,
        }))
      }
    } catch (err) {
      console.error("加载任务日志失败:", err)
      setError(
        err instanceof Error ? err.message : "加载任务日志失败"
      )
    } finally {
      setLoading(false)
    }
  }, [filters, pagination])

  const refreshLogs = useCallback(async () => {
    await loadLogs()
  }, [loadLogs])

  const updateFilters = useCallback(
    (updates: Partial<LogFilters>) => {
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

  const searchLogs = useCallback(
    async (query: string) => {
      updateFilters({ search: query })
    },
    [updateFilters]
  )

  const levelOptions = useMemo<LogLevel[]>(
    () => ["info", "warn", "warning", "error", "debug", "critical"],
    []
  )

  useEffect(() => {
    if (taskId) {
      setFiltersState((prev) => ({
        ...prev,
        taskId,
      }))
    }
  }, [taskId])

  useEffect(() => {
    void loadLogs()
  }, [loadLogs])

  return {
    logs,
    loading,
    error,
    filters,
    pagination,
    loadLogs,
    refreshLogs,
    setFilters: updateFilters,
    setPagination: updatePagination,
    searchLogs,
    levelOptions,
  }
}
