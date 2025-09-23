import { useCallback, useEffect, useState } from "react"

import { fetchDeploymentSiteDetail } from "@/lib/api"
import type { DeploymentSiteDetail } from "@/types/api"

export function useDeploymentSiteDetail(id: string | null) {
  const [data, setData] = useState<DeploymentSiteDetail | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [reloadKey, setReloadKey] = useState(0)

  const load = useCallback(
    async (siteId: string) => {
      setLoading(true)
      try {
        const response = await fetchDeploymentSiteDetail(siteId)
        setData(response.data ?? null)
        setError(null)
      } catch (err) {
        setError(err instanceof Error ? err.message : "加载部署站点详情失败")
      } finally {
        setLoading(false)
      }
    },
    [],
  )

  useEffect(() => {
    if (!id) {
      setData(null)
      setError(null)
      return
    }
    void load(id)
  }, [id, load, reloadKey])

  const refresh = useCallback(() => {
    if (!id) return
    setReloadKey((key) => key + 1)
  }, [id])

  return { data, loading, error, refresh }
}
