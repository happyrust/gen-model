import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  listSites,
  getSite,
  createSite,
  updateSite,
  deleteSite,
} from '@/lib/api/remote-sync'
import type { Site } from '@/types/remote-sync'

/**
 * 查询站点列表
 */
export function useSites(envId: string) {
  return useQuery({
    queryKey: ['sites', envId],
    queryFn: () => listSites(envId),
    enabled: !!envId,
  })
}

/**
 * 查询单个站点
 */
export function useSite(siteId: string) {
  return useQuery({
    queryKey: ['sites', 'detail', siteId],
    queryFn: () => getSite(siteId),
    enabled: !!siteId,
  })
}

/**
 * 创建站点
 */
export function useCreateSite() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: ({ envId, data }: { envId: string; data: Partial<Site> }) =>
      createSite(envId, data),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: ['sites', variables.envId] })
    },
  })
}

/**
 * 更新站点
 */
export function useUpdateSite() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: ({ siteId, data }: { siteId: string; data: Partial<Site> }) =>
      updateSite(siteId, data),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: ['sites'] })
      queryClient.invalidateQueries({ queryKey: ['sites', 'detail', variables.siteId] })
    },
  })
}

/**
 * 删除站点
 */
export function useDeleteSite() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: deleteSite,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['sites'] })
    },
  })
}
