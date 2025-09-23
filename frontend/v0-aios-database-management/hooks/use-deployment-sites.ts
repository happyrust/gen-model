import { useCallback, useEffect, useMemo, useState } from "react"

import { fetchDeploymentSites } from "@/lib/api"
import type { DeploymentSite, DeploymentSiteFilters, DeploymentSitesResponse } from "@/types/api"

import { useDebouncedValue } from "@/hooks/use-debounced-value"

const DEFAULT_FILTERS: DeploymentSiteFilters = {
  q: "",
  status: "",
  env: "",
  owner: "",
  sort: "updated_at:desc",
  page: 1,
  per_page: 6,
}

export interface DeploymentSiteStats {
  total: number
  running: number
  deploying: number
  configuring: number
  failed: number
}

export interface UseDeploymentSitesResult {
  items: DeploymentSite[]
  loading: boolean
  error: string | null
  pagination: {
    page: number
    perPage: number
    total: number
    pages: number
  }
  stats: DeploymentSiteStats
  filters: DeploymentSiteFilters
  setFilter: (updates: Partial<DeploymentSiteFilters>) => void
  setPage: (page: number) => void
  refresh: () => void
}

function computeStats(items: DeploymentSite[], total: number): DeploymentSiteStats {
  const normalize = (status?: string) => status?.toLowerCase() ?? ""
  const counts = items.reduce(
    (acc, site) => {
      const status = normalize(site.status)
      if (status.includes("run") || status.includes("active")) acc.running += 1
      else if (status.includes("deploy")) acc.deploying += 1
      else if (status.includes("config")) acc.configuring += 1
      else if (status.includes("fail") || status.includes("error")) acc.failed += 1
      return acc
    },
    { running: 0, deploying: 0, configuring: 0, failed: 0 },
  )
  return {
    total,
    running: counts.running,
    deploying: counts.deploying,
    configuring: counts.configuring,
    failed: counts.failed,
  }
}

export function useDeploymentSites(initialFilters: DeploymentSiteFilters = {}): UseDeploymentSitesResult {
  const [filters, setFilters] = useState<DeploymentSiteFilters>({
    ...DEFAULT_FILTERS,
    ...initialFilters,
  })
  const debouncedFilters = useDebouncedValue(filters, 300)

  const [data, setData] = useState<DeploymentSitesResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [reloadKey, setReloadKey] = useState(0)

  const fetchData = useCallback(async () => {
    setLoading(true)
    try {
      const response = await fetchDeploymentSites(debouncedFilters)
      setData(response)
      setError(null)
    } catch (err) {
      const message = err instanceof Error ? err.message : "获取部署站点失败"
      setError(message)
    } finally {
      setLoading(false)
    }
  }, [debouncedFilters])

  useEffect(() => {
    void fetchData()
  }, [fetchData, reloadKey])

  const items = data?.items ?? []
  const pagination = {
    page: data?.page ?? filters.page ?? 1,
    perPage: data?.per_page ?? filters.per_page ?? DEFAULT_FILTERS.per_page ?? 6,
    total: data?.total ?? items.length,
    pages: data?.pages ?? Math.max(1, Math.ceil((data?.total ?? items.length) / (filters.per_page || DEFAULT_FILTERS.per_page || 6))),
  }

  const stats = useMemo(() => computeStats(items, pagination.total), [items, pagination.total])

  const setFilter = useCallback((updates: Partial<DeploymentSiteFilters>) => {
    setFilters((prev) => ({
      ...prev,
      ...updates,
      page: updates.page ?? 1,
    }))
  }, [])

  const setPage = useCallback((page: number) => {
    setFilters((prev) => ({ ...prev, page }))
  }, [])

  return {
    items,
    loading,
    error,
    pagination,
    stats,
    filters,
    setFilter,
    setPage,
    refresh: () => setReloadKey((key) => key + 1),
  }
}
