"use client"

import { useState, useCallback, useMemo } from "react"
import type { Task } from "@/types/task-monitor"

export function useBatchSelection(tasks: Task[]) {
  const [selectedTasks, setSelectedTasks] = useState<string[]>([])

  // 全选
  const selectAll = useCallback(() => {
    setSelectedTasks(tasks.map(task => task.id))
  }, [tasks])

  // 取消全选
  const selectNone = useCallback(() => {
    setSelectedTasks([])
  }, [])

  // 切换单个任务选择
  const toggleTask = useCallback((taskId: string) => {
    setSelectedTasks(prev => 
      prev.includes(taskId) 
        ? prev.filter(id => id !== taskId)
        : [...prev, taskId]
    )
  }, [])

  // 检查任务是否被选中
  const isSelected = useCallback((taskId: string) => {
    return selectedTasks.includes(taskId)
  }, [selectedTasks])

  // 检查是否全选
  const isAllSelected = useMemo(() => {
    return tasks.length > 0 && selectedTasks.length === tasks.length
  }, [tasks.length, selectedTasks.length])

  // 检查是否部分选择
  const isIndeterminate = useMemo(() => {
    return selectedTasks.length > 0 && selectedTasks.length < tasks.length
  }, [selectedTasks.length, tasks.length])

  // 获取选中的任务对象
  const selectedTaskObjects = useMemo(() => {
    return tasks.filter(task => selectedTasks.includes(task.id))
  }, [tasks, selectedTasks])

  // 按状态分组选中的任务
  const selectedTasksByStatus = useMemo(() => {
    const grouped = {
      pending: [] as Task[],
      running: [] as Task[],
      completed: [] as Task[],
      failed: [] as Task[],
      paused: [] as Task[],
      cancelled: [] as Task[]
    }

    selectedTaskObjects.forEach(task => {
      if (grouped[task.status as keyof typeof grouped]) {
        grouped[task.status as keyof typeof grouped].push(task)
      }
    })

    return grouped
  }, [selectedTaskObjects])

  // 按类型分组选中的任务
  const selectedTasksByType = useMemo(() => {
    const grouped = {
      ModelGeneration: [] as Task[],
      SpatialTreeGeneration: [] as Task[],
      FullSync: [] as Task[],
      IncrementalSync: [] as Task[]
    }

    selectedTaskObjects.forEach(task => {
      if (grouped[task.type as keyof typeof grouped]) {
        grouped[task.type as keyof typeof grouped].push(task)
      }
    })

    return grouped
  }, [selectedTaskObjects])

  // 批量选择特定状态的任务
  const selectByStatus = useCallback((status: string) => {
    const tasksWithStatus = tasks.filter(task => task.status === status)
    setSelectedTasks(tasksWithStatus.map(task => task.id))
  }, [tasks])

  // 批量选择特定类型的任务
  const selectByType = useCallback((type: string) => {
    const tasksWithType = tasks.filter(task => task.type === type)
    setSelectedTasks(tasksWithType.map(task => task.id))
  }, [tasks])

  // 清除选择
  const clearSelection = useCallback(() => {
    setSelectedTasks([])
  }, [])

  // 反选
  const invertSelection = useCallback(() => {
    const unselectedTasks = tasks.filter(task => !selectedTasks.includes(task.id))
    setSelectedTasks(unselectedTasks.map(task => task.id))
  }, [tasks, selectedTasks])

  return {
    selectedTasks,
    selectedTaskObjects,
    selectedTasksByStatus,
    selectedTasksByType,
    selectAll,
    selectNone,
    toggleTask,
    isSelected,
    isAllSelected,
    isIndeterminate,
    selectByStatus,
    selectByType,
    clearSelection,
    invertSelection
  }
}
