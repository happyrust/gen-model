"use client"

import { useState, useEffect, useCallback } from "react"
import { fetchTaskHistory, replayTask } from "@/lib/api/task-history"
import type { TaskHistory, HistoryFilters, PaginationState } from "@/types/task-history"

interface TaskHistoryState {
  tasks: TaskHistory[]
  loading: boolean
  error: string | null
  filters: HistoryFilters
  pagination: PaginationState
}

export function useTaskHistory() {
  const [state, setState] = useState<TaskHistoryState>({
    tasks: [],
    loading: false,
    error: null,
    filters: {
      status: 'all',
      type: 'all',
      search: '',
      dateRange: null,
      sortBy: 'startTime',
      sortOrder: 'desc'
    },
    pagination: {
      currentPage: 1,
      pageSize: 20,
      totalPages: 1,
      totalItems: 0
    }
  })

  // 加载历史数据
  const loadHistory = useCallback(async (filters: HistoryFilters) => {
    setState(prev => ({ ...prev, loading: true, error: null }))

    try {
      const response = await fetchTaskHistory({
        ...filters,
        page: state.pagination.currentPage,
        pageSize: state.pagination.pageSize
      })

      setState(prev => ({
        ...prev,
        tasks: response.tasks,
        pagination: {
          ...prev.pagination,
          totalPages: response.pagination.totalPages,
          totalItems: response.pagination.totalItems
        },
        loading: false
      }))
    } catch (error) {
      console.error('Failed to load task history:', error)
      setState(prev => ({
        ...prev,
        error: error instanceof Error ? error.message : '加载历史数据失败',
        loading: false
      }))
    }
  }, [state.pagination.currentPage, state.pagination.pageSize])

  // 刷新历史数据
  const refreshHistory = useCallback(async () => {
    await loadHistory(state.filters)
  }, [loadHistory, state.filters])

  // 重新执行任务
  const replayTaskHistory = useCallback(async (taskId: string, parameters?: Record<string, any>) => {
    try {
      const response = await replayTask(taskId, parameters)
      await refreshHistory() // 刷新列表
      return response
    } catch (error) {
      console.error('Failed to replay task:', error)
      setState(prev => ({
        ...prev,
        error: error instanceof Error ? error.message : '重新执行任务失败'
      }))
      throw error
    }
  }, [refreshHistory])

  // 设置过滤条件
  const setFilters = useCallback((newFilters: Partial<HistoryFilters>) => {
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

  // 获取任务详情
  const getTaskDetails = useCallback((taskId: string) => {
    return state.tasks.find(task => task.id === taskId)
  }, [state.tasks])

  // 获取任务统计
  const getTaskStatistics = useCallback(() => {
    const tasks = state.tasks
    const total = tasks.length
    const completed = tasks.filter(t => t.status === 'completed').length
    const failed = tasks.filter(t => t.status === 'failed').length
    const cancelled = tasks.filter(t => t.status === 'cancelled').length
    const running = tasks.filter(t => t.status === 'running').length

    const successRate = total > 0 ? (completed / total) * 100 : 0
    const failureRate = total > 0 ? (failed / total) * 100 : 0

    const avgDuration = tasks.length > 0 
      ? tasks.reduce((sum, task) => sum + (task.duration || 0), 0) / tasks.length 
      : 0

    return {
      total,
      completed,
      failed,
      cancelled,
      running,
      successRate,
      failureRate,
      avgDuration
    }
  }, [state.tasks])

  // 按类型分组统计
  const getStatisticsByType = useCallback(() => {
    const tasks = state.tasks
    const grouped = {
      ModelGeneration: tasks.filter(t => t.type === 'ModelGeneration'),
      SpatialTreeGeneration: tasks.filter(t => t.type === 'SpatialTreeGeneration'),
      FullSync: tasks.filter(t => t.type === 'FullSync'),
      IncrementalSync: tasks.filter(t => t.type === 'IncrementalSync')
    }

    return Object.entries(grouped).map(([type, typeTasks]) => ({
      type,
      total: typeTasks.length,
      completed: typeTasks.filter(t => t.status === 'completed').length,
      failed: typeTasks.filter(t => t.status === 'failed').length,
      avgDuration: typeTasks.length > 0 
        ? typeTasks.reduce((sum, task) => sum + (task.duration || 0), 0) / typeTasks.length 
        : 0
    }))
  }, [state.tasks])

  // 按日期分组统计
  const getStatisticsByDate = useCallback((days: number = 7) => {
    const tasks = state.tasks
    const endDate = new Date()
    const startDate = new Date(endDate.getTime() - days * 24 * 60 * 60 * 1000)

    const filteredTasks = tasks.filter(task => {
      const taskDate = new Date(task.startTime)
      return taskDate >= startDate && taskDate <= endDate
    })

    const dailyStats = []
    for (let i = 0; i < days; i++) {
      const date = new Date(endDate.getTime() - i * 24 * 60 * 60 * 1000)
      const dayTasks = filteredTasks.filter(task => {
        const taskDate = new Date(task.startTime)
        return taskDate.toDateString() === date.toDateString()
      })

      dailyStats.push({
        date: date.toISOString().split('T')[0],
        total: dayTasks.length,
        completed: dayTasks.filter(t => t.status === 'completed').length,
        failed: dayTasks.filter(t => t.status === 'failed').length
      })
    }

    return dailyStats.reverse()
  }, [state.tasks])

  return {
    ...state,
    loadHistory,
    refreshHistory,
    replayTask: replayTaskHistory,
    setFilters,
    setPagination,
    getTaskDetails,
    getTaskStatistics,
    getStatisticsByType,
    getStatisticsByDate
  }
}
