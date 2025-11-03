"use client"

import { useState, useEffect, useCallback } from "react"
import { fetchTaskLogs, searchLogs } from "@/lib/api/task-logs"
import type { LogEntry, LogFilters, PaginationState } from "@/types/task-logs"

interface TaskLogsState {
  logs: LogEntry[]
  loading: boolean
  error: string | null
  filters: LogFilters
  pagination: PaginationState
}

export function useTaskLogs(taskId?: string) {
  const [state, setState] = useState<TaskLogsState>({
    logs: [],
    loading: false,
    error: null,
    filters: {
      level: 'all',
      search: '',
      dateRange: null,
      taskId: taskId || 'all'
    },
    pagination: {
      currentPage: 1,
      pageSize: 100,
      totalPages: 1,
      totalItems: 0
    }
  })

  // 加载日志
  const loadLogs = useCallback(async (filters: LogFilters) => {
    setState(prev => ({ ...prev, loading: true, error: null }))

    try {
      const response = await fetchTaskLogs({
        taskId: filters.taskId === 'all' ? undefined : filters.taskId,
        level: filters.level === 'all' ? undefined : filters.level,
        search: filters.search || undefined,
        dateRange: filters.dateRange,
        page: state.pagination.currentPage,
        pageSize: state.pagination.pageSize
      })

      setState(prev => ({
        ...prev,
        logs: response.logs,
        pagination: {
          ...prev.pagination,
          totalPages: response.pagination.totalPages,
          totalItems: response.pagination.totalItems
        },
        loading: false
      }))
    } catch (error) {
      console.error('Failed to load task logs:', error)
      setState(prev => ({
        ...prev,
        error: error instanceof Error ? error.message : '加载日志失败',
        loading: false
      }))
    }
  }, [state.pagination.currentPage, state.pagination.pageSize])

  // 刷新日志
  const refreshLogs = useCallback(async () => {
    await loadLogs(state.filters)
  }, [loadLogs, state.filters])

  // 搜索日志
  const searchTaskLogs = useCallback(async (query: string) => {
    if (!query.trim()) {
      await loadLogs(state.filters)
      return
    }

    setState(prev => ({ ...prev, loading: true, error: null }))

    try {
      const response = await searchLogs({
        query,
        taskId: state.filters.taskId === 'all' ? undefined : state.filters.taskId,
        level: state.filters.level === 'all' ? undefined : state.filters.level,
        dateRange: state.filters.dateRange
      })

      setState(prev => ({
        ...prev,
        logs: response.logs,
        loading: false
      }))
    } catch (error) {
      console.error('Failed to search logs:', error)
      setState(prev => ({
        ...prev,
        error: error instanceof Error ? error.message : '搜索日志失败',
        loading: false
      }))
    }
  }, [state.filters, loadLogs])

  // 设置过滤条件
  const setFilters = useCallback((newFilters: Partial<LogFilters>) => {
    setState(prev => ({
      ...prev,
      filters: { ...prev.filters, ...newFilters }
    }))
  }, [])

  // 设置分页
  const setPagination = useCallback((newPagination: Partial<PaginationState>) => {
    setState(prev => ({
      ...prev,
      pagination: { ...prev.pagination, ...newPagination }
    }))
  }, [])

  // 初始化加载
  useEffect(() => {
    if (taskId) {
      setState(prev => ({
        ...prev,
        filters: { ...prev.filters, taskId }
      }))
    }
  }, [taskId])

  // 过滤条件变化时重新加载
  useEffect(() => {
    loadLogs(state.filters)
  }, [loadLogs, state.filters])

  return {
    ...state,
    loadLogs,
    refreshLogs,
    searchLogs: searchTaskLogs,
    setFilters,
    setPagination
  }
}
