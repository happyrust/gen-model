import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  listEnvironments,
  getEnvironment,
  createEnvironment,
  updateEnvironment,
  deleteEnvironment,
  activateEnvironment,
} from '@/lib/api/remote-sync'
import type { Environment } from '@/types/remote-sync'

/**
 * 查询环境列表
 */
export function useEnvironments() {
  return useQuery({
    queryKey: ['environments'],
    queryFn: listEnvironments,
  })
}

/**
 * 查询单个环境
 */
export function useEnvironment(envId: string) {
  return useQuery({
    queryKey: ['environments', envId],
    queryFn: () => getEnvironment(envId),
    enabled: !!envId,
  })
}

/**
 * 创建环境
 */
export function useCreateEnvironment() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: createEnvironment,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['environments'] })
    },
  })
}

/**
 * 更新环境
 */
export function useUpdateEnvironment() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: ({ envId, data }: { envId: string; data: Partial<Environment> }) =>
      updateEnvironment(envId, data),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: ['environments'] })
      queryClient.invalidateQueries({ queryKey: ['environments', variables.envId] })
    },
  })
}

/**
 * 删除环境
 */
export function useDeleteEnvironment() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: deleteEnvironment,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['environments'] })
    },
  })
}

/**
 * 激活环境
 */
export function useActivateEnvironment() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: activateEnvironment,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['environments'] })
      queryClient.invalidateQueries({ queryKey: ['sync-status'] })
    },
  })
}
