"use client"

import { useState, useEffect, useCallback } from "react"
import { fetchTaskStatus, startTask, stopTask, pauseTask } from "@/lib/api/task-monitor"
import type { Task, SystemMetrics } from "@/types/task-monitor"

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
      disk: 0,
      network: 0
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
        tasks: response.tasks || [],
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
