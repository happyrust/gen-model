import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  startSync,
  stopSync,
  restartSync,
  pauseSync,
  resumeSync,
  getSyncStatus,
  getMetrics,
  getTaskQueue,
  clearQueue,
  addSyncTask,
  cancelTask,
} from '@/lib/api/remote-sync'
import type { SyncTask } from '@/types/remote-sync'

/**
 * 查询同步状态
 */
export function useSyncStatus() {
  return useQuery({
    queryKey: ['sync-status'],
    queryFn: getSyncStatus,
    refetchInterval: 5000, // 每 5 秒刷新一次
  })
}

/**
 * 查询性能指标
 */
export function useMetrics() {
  return useQuery({
    queryKey: ['metrics'],
    queryFn: getMetrics,
    refetchInterval: 3000, // 每 3 秒刷新一次
  })
}

/**
 * 查询任务队列
 */
export function useTaskQueue() {
  return useQuery({
    queryKey: ['task-queue'],
    queryFn: getTaskQueue,
    refetchInterval: 2000, // 每 2 秒刷新一次
  })
}

/**
 * 启动同步
 */
export function useStartSync() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: startSync,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['sync-status'] })
    },
  })
}

/**
 * 停止同步
 */
export function useStopSync() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: stopSync,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['sync-status'] })
    },
  })
}

/**
 * 重启同步
 */
export function useRestartSync() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: restartSync,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['sync-status'] })
    },
  })
}

/**
 * 暂停同步
 */
export function usePauseSync() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: pauseSync,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['sync-status'] })
    },
  })
}

/**
 * 恢复同步
 */
export function useResumeSync() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: resumeSync,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['sync-status'] })
    },
  })
}

/**
 * 清空队列
 */
export function useClearQueue() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: clearQueue,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['task-queue'] })
    },
  })
}

/**
 * 添加同步任务
 */
export function useAddSyncTask() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: addSyncTask,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['task-queue'] })
    },
  })
}

/**
 * 取消任务
 */
export function useCancelTask() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: cancelTask,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['task-queue'] })
    },
  })
}
