/**
 * 部署站点数据获取 Hook
 *
 * 负责从 API 获取站点数据，处理加载状态和错误
 */

import { useCallback, useEffect, useState, useMemo } from "react"
import type { Site } from "@/components/deployment-sites/site-card"
import { deleteDeploymentSite, fetchDeploymentSites, patchDeploymentSite } from "@/lib/api"
import { mapSites } from "../utils/site-mappers"
import type { SiteFilters } from "./use-site-filters"
import { fetchParsingTaskStatus as fetchParsingStatus } from "@/lib/parsing-apis"
import { fetchModelGenerationTaskStatus as fetchModelStatus } from "@/lib/model-generation-apis"

export interface SiteStats {
  total: number
  running: number
  deploying: number
  configuring: number
  failed: number
  paused: number
}

export interface UseDeploymentSitesReturn {
  sites: Site[]
  total: number
  loading: boolean
  initialized: boolean
  error: string | null
  stats: SiteStats
  totalPages: number
  refetch: () => Promise<{ success: boolean; error?: string }>
  addSite: (site: Site) => void
  updateSiteStatus: (siteId: string, status: Site["status"]) => Promise<void>
  removeSite: (siteId: string) => Promise<void>
}

/**
 * 使用部署站点数据
 */
export function useDeploymentSites(filters: SiteFilters): UseDeploymentSitesReturn {
  const [sites, setSites] = useState<Site[]>([])
  const [total, setTotal] = useState(0)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [initialized, setInitialized] = useState(false)

  // 获取站点列表
  const fetchSites = useCallback(async (): Promise<{ success: boolean; error?: string }> => {
    setLoading(true)
    setError(null)

    try {
      const response = await fetchDeploymentSites({
        q: filters.search || undefined,
        status: filters.status || undefined,
        env: filters.environment || undefined,
        owner: filters.owner || undefined,
        sort: filters.sort || undefined,
        page: filters.page,
        per_page: filters.perPage,
      })

      const parsedSites = mapSites(response.items as Array<Record<string, unknown>>)
      setSites(parsedSites)
      setTotal(response.total)

      // 异步补充解析/模型生成状态（不阻塞首屏）
      ;(async () => {
        // 限制并发，避免同时请求过多
        const concurrency = 6
        let index = 0
        const nextBatch = async () => {
          const batch = parsedSites.slice(index, index + concurrency)
          index += concurrency
          await Promise.all(
            batch.map(async (site) => {
              try {
                // 解析状态
                const parsing = await fetchParsingStatus(site.id)
                // 模型生成状态
                const model = await fetchModelStatus(site.id)

                setSites((prev) =>
                  prev.map((s) =>
                    s.id === site.id
                      ? {
                          ...s,
                          parsingStatus:
                            (parsing?.status as Site["parsingStatus"]) ?? s.parsingStatus ?? "unknown",
                          modelGenerationStatus:
                            (model?.status as Site["modelGenerationStatus"]) ?? s.modelGenerationStatus ?? "unknown",
                        }
                      : s,
                  ),
                )
              } catch {
                // 静默失败，保持未知
              }
            }),
          )
          if (index < parsedSites.length) {
            await nextBatch()
          }
        }
        if (parsedSites.length > 0) {
          await nextBatch()
        }
      })()

      return { success: true as const }
    } catch (err) {
      const message = err instanceof Error ? err.message : "加载部署站点失败"
      setError(message)
      setSites([])
      setTotal(0)
      return { success: false as const, error: message }
    } finally {
      setLoading(false)
      setInitialized(true)
    }
  }, [filters])

  // 自动获取数据
  useEffect(() => {
    fetchSites()
  }, [filters])

  // 计算站点统计信息
  const stats = useMemo<SiteStats>(() => {
    const running = sites.filter((s) => s.status === "running").length
    const deploying = sites.filter((s) => s.status === "deploying").length
    const configuring = sites.filter((s) => s.status === "configuring").length
    const failed = sites.filter((s) => s.status === "failed").length
    const paused = sites.filter((s) => s.status === "paused" || s.status === "stopped").length

    return {
      total,
      running,
      deploying,
      configuring,
      failed,
      paused,
    }
  }, [sites, total])

  // 计算总页数
  const totalPages = Math.max(1, Math.ceil(total / filters.perPage))

  // 添加新站点（乐观更新）
  const addSite = useCallback((site: Site) => {
    setSites((prev) => [site, ...prev])
    setTotal((prev) => prev + 1)
    // 可选：刷新以获取服务器最新数据
    fetchSites()
  }, [fetchSites])

  // 更新站点信息（乐观更新）
  const updateSiteStatus = useCallback(async (siteId: string, status: Site["status"]) => {
    let previousStatus: Site["status"] | null = null
    let siteFound = false

    setSites((prev) =>
      prev.map((s) => {
        if (s.id === siteId) {
          siteFound = true
          previousStatus = s.status
          return { ...s, status }
        }
        return s
      }),
    )

    if (!siteFound || previousStatus === null) {
      throw new Error("站点不存在或已被移除")
    }

    try {
      await patchDeploymentSite(siteId, { status })
      const result = await fetchSites()
      if (!result.success) {
        throw new Error(result.error || "刷新站点状态失败")
      }
    } catch (error) {
      const restoreStatus = previousStatus
      setSites((prev) => prev.map((s) => (s.id === siteId ? { ...s, status: restoreStatus } : s)))
      throw error
    }
  }, [fetchSites])

  // 删除站点（乐观更新）
  const removeSite = useCallback(async (siteId: string) => {
    let removedSite: Site | null = null
    let removedIndex = -1

    setSites((prev) => {
      const next: Site[] = []
      prev.forEach((site, index) => {
        if (site.id === siteId) {
          removedSite = site
          removedIndex = index
        } else {
          next.push(site)
        }
      })
      return next
    })

    if (!removedSite) {
      throw new Error("站点不存在或已被移除")
    }

    setTotal((prev) => Math.max(0, prev - 1))

    try {
      await deleteDeploymentSite(siteId)
      const result = await fetchSites()
      if (!result.success) {
        throw new Error(result.error || "刷新站点列表失败")
      }
    } catch (error) {
      const restore = removedSite
      setSites((prev) => {
        const next = [...prev]
        next.splice(removedIndex, 0, restore)
        return next
      })
      setTotal((prev) => prev + 1)
      throw error
    }
  }, [fetchSites])

  return {
    sites,
    total,
    loading,
    initialized,
    error,
    stats,
    totalPages,
    refetch: fetchSites,
    addSite,
    updateSiteStatus,
    removeSite,
  }
}
