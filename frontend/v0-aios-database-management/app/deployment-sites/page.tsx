"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { Button } from "@/components/ui/button"
import { RefreshCw, Share2, Database, FilePlus } from "lucide-react"
import { Sidebar } from "@/components/sidebar"
import { StatsPanel } from "@/components/deployment-sites/stats-panel"
import { FiltersBar } from "@/components/deployment-sites/filters-bar"
import { SiteList } from "@/components/deployment-sites/site-list"
import type { Site } from "@/components/deployment-sites/site-card"
import { fetchDeploymentSites } from "@/lib/api"
import { EnhancedCreateSiteDialog } from "@/components/deployment-sites/enhanced-create-site-dialog"

export default function DeploymentSitesPage() {
  const [sites, setSites] = useState<Site[]>([])
  const [total, setTotal] = useState(0)
  const [page, setPage] = useState(1)
  const [perPage, setPerPage] = useState(12)
  const [search, setSearch] = useState("")
  const [status, setStatus] = useState("")
  const [environment, setEnvironment] = useState("")
  const [owner, setOwner] = useState("")
  const [sort, setSort] = useState("updated_at:desc")
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [viewMode, setViewMode] = useState<"grid" | "list">("grid")

  const mapStatus = (value: unknown): Site["status"] => {
    if (typeof value === "string") {
      const normalized = value.toLowerCase()
      if (normalized === "stopped") return "stopped"
      if (normalized === "paused") return "paused"
      if (normalized === "running") return "running"
      if (normalized === "deploying") return "deploying"
      if (normalized === "configuring") return "configuring"
      if (normalized === "failed") return "failed"
    }
    return "configuring"
  }

  const mapEnvironment = (value: unknown): Site["environment"] => {
    if (typeof value === "string") {
      const normalized = value.toLowerCase()
      if (["dev", "test", "staging", "prod"].includes(normalized)) {
        return normalized as Site["environment"]
      }
    }
    return "dev"
  }

  const mapDate = (value: unknown) => {
    if (!value) return new Date().toISOString()
    if (typeof value === "string") return value
    if (typeof value === "number") {
      if (value > 1_000_000_000_000) {
        return new Date(value).toISOString()
      }
      return new Date(value * 1000).toISOString()
    }
    return new Date().toISOString()
  }

  const mapSite = (item: Record<string, unknown>): Site => ({
    id:
      typeof item.id === "string"
        ? item.id
        : typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `site-${Date.now()}`,
    name: (item.name as string) || "未命名站点",
    status: mapStatus(item.status),
    environment: mapEnvironment(item.env),
    owner: (item.owner as string) || undefined,
    createdAt: mapDate(item.created_at),
    updatedAt: mapDate(item.updated_at),
    url: typeof item.url === "string" ? item.url : undefined,
    description: typeof item.description === "string" ? item.description : undefined,
  })

  const fetchSites = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const response = await fetchDeploymentSites({
        q: search || undefined,
        status: status || undefined,
        env: environment || undefined,
        owner: owner || undefined,
        sort: sort || undefined,
        page,
        per_page: perPage,
      })

      const parsedSites = response.items.map((item) => mapSite(item as Record<string, unknown>))
      setSites(parsedSites)
      setTotal(response.total)
    } catch (err) {
      setError(err instanceof Error ? err.message : "加载部署站点失败")
      setSites([])
      setTotal(0)
    } finally {
      setLoading(false)
    }
  }, [search, status, environment, owner, sort, page, perPage])

  useEffect(() => {
    fetchSites()
  }, [fetchSites])

  const siteStats = useMemo(() => {
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

  const totalPages = Math.max(1, Math.ceil(total / perPage))

  const handleRefresh = () => {
    fetchSites()
  }

  const handleCopyShareLink = () => {
    if (typeof window === "undefined" || !navigator?.clipboard) return
    const url = window.location.href
    navigator.clipboard.writeText(url)
  }

  const handleCreateSite = (site: Site) => {
    // 添加新站点到列表并刷新
    setSites((prev) => [site, ...prev])
    setTotal(prev => prev + 1)
    // 可选：刷新列表以获取最新数据
    fetchSites()
  }

  const handleSiteView = (site: Site) => {
    console.log("查看站点:", site.name)
  }

  const handleSiteStart = (site: Site) => {
    setSites((prev) => prev.map((s) => (s.id === site.id ? { ...s, status: "running" as const } : s)))
  }

  const handleSitePause = (site: Site) => {
    setSites((prev) => prev.map((s) => (s.id === site.id ? { ...s, status: "paused" as const } : s)))
  }

  const handleSiteConfigure = (site: Site) => {
    console.log("配置站点:", site.name)
  }

  const handleSiteDelete = (site: Site) => {
    setSites((prev) => prev.filter((s) => s.id !== site.id))
  }

  return (
    <div className="min-h-screen bg-background">
      <Sidebar />

      <div className="ml-64 p-8">
        <div className="space-y-6">
          {/* Header */}
          <div className="flex items-center justify-between">
            <div className="flex items-center space-x-2">
              <Database className="h-6 w-6" />
              <h1 className="text-2xl font-bold">部署站点管理</h1>
            </div>
            <div className="flex items-center space-x-2">
              <Button
                variant="outline"
                size="sm"
                onClick={() => console.log("从 DbOption 导入")}
                className="gap-1"
              >
                <FilePlus className="h-4 w-4" />
                从 DbOption 导入
              </Button>
              <Button variant="outline" size="sm" onClick={handleCopyShareLink} className="gap-1">
                <Share2 className="h-4 w-4" />
                复制分享链接
              </Button>
              <Button variant="outline" size="sm" onClick={handleRefresh} className="gap-1">
                <RefreshCw className={`h-4 w-4 ${loading ? "animate-spin" : ""}`} />
                刷新
              </Button>
              <EnhancedCreateSiteDialog onCreateSite={handleCreateSite} />
            </div>
          </div>

          {/* Statistics Cards */}
          <StatsPanel stats={siteStats} />

          {/* Filters and Search */}
          <FiltersBar
            search={search}
            status={status}
            environment={environment}
            owner={owner}
            sort={sort}
            pageSize={perPage}
            total={total}
            viewMode={viewMode}
            onSearchChange={(value) => {
              setSearch(value)
              setPage(1)
            }}
            onStatusChange={(value) => {
              setStatus(value)
              setPage(1)
            }}
            onEnvironmentChange={(value) => {
              setEnvironment(value)
              setPage(1)
            }}
            onOwnerChange={(value) => {
              setOwner(value)
              setPage(1)
            }}
            onSortChange={(value) => {
              setSort(value)
            }}
            onPageSizeChange={(value) => {
              const size = parseInt(value, 10)
              setPerPage(Number.isNaN(size) ? 12 : size)
              setPage(1)
            }}
            onViewModeChange={(mode) => setViewMode(mode)}
          />

          {error && <div className="rounded-md border border-destructive/50 bg-destructive/10 px-4 py-3 text-sm text-destructive">{error}</div>}

          {loading && (
            <div className="rounded-md border border-border bg-muted/40 px-4 py-3 text-sm text-muted-foreground">
              正在加载部署站点...
            </div>
          )}

          {/* Site List */}
          <SiteList
            sites={sites}
            viewMode={viewMode}
            onSiteView={handleSiteView}
            onSiteStart={handleSiteStart}
            onSitePause={handleSitePause}
            onSiteConfigure={handleSiteConfigure}
            onSiteDelete={handleSiteDelete}
          />

          <div className="flex items-center justify-between text-sm text-muted-foreground border-t border-border pt-4">
            <span>
              第 {page} / {totalPages} 页
            </span>
            <div className="flex items-center gap-2">
              <Button variant="outline" size="sm" disabled={page <= 1 || loading} onClick={() => setPage((prev) => Math.max(1, prev - 1))}>
                上一页
              </Button>
              <Button
                variant="outline"
                size="sm"
                disabled={page >= totalPages || loading}
                onClick={() => setPage((prev) => Math.min(totalPages, prev + 1))}
              >
                下一页
              </Button>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}
