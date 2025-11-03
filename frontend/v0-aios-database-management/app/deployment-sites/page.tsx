"use client"

/**
 * 部署站点管理页面
 *
 * 主要职责：
 * - 页面布局和组件组合
 * - 事件处理分发
 */

import { Button } from "@/components/ui/button"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { RefreshCw, Share2, Database, FilePlus, AlertCircle } from "lucide-react"
import { Sidebar } from "@/components/sidebar"
import { StatsPanel } from "@/components/deployment-sites/stats-panel"
import { FiltersBar } from "@/components/deployment-sites/filters-bar"
import { SiteList } from "@/components/deployment-sites/site-list"
import { EnhancedCreateSiteDialog } from "@/components/deployment-sites/enhanced-create-site-dialog"
import type { Site } from "@/components/deployment-sites/site-card"
import { toast } from "sonner"

import { useSiteFilters } from "./hooks/use-site-filters"
import { useDeploymentSites } from "./hooks/use-deployment-sites"

export default function DeploymentSitesPage() {
  // 过滤器状态
  const { filters, actions } = useSiteFilters()

  // 站点数据
  const { sites, total, loading, initialized, error, stats, totalPages, refetch, addSite, updateSiteStatus, removeSite } =
    useDeploymentSites(filters)

  // 事件处理
  const handleRefresh = async () => {
    const { success, error: refreshError } = await refetch()
    if (success) {
      toast.success("已刷新最新站点数据")
    } else if (refreshError) {
      toast.error(refreshError)
    }
  }

  const handleCopyShareLink = async () => {
    if (typeof window === "undefined" || !navigator?.clipboard) {
      toast.error("当前环境不支持复制链接")
      return
    }

    try {
      await navigator.clipboard.writeText(window.location.href)
      toast.success("链接已复制到剪贴板")
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "复制链接失败")
    }
  }

  const handleSiteView = (site: Site) => {
    console.log("查看站点:", site.name)
    // 这个函数现在不再需要，因为站点卡片直接处理导航
  }

  const handleSiteStart = async (site: Site) => {
    const toastId = toast.loading(`正在启动 ${site.name}...`)
    try {
      await updateSiteStatus(site.id, "running")
      toast.success(`${site.name} 已启动`, { id: toastId })
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "无法启动站点", { id: toastId })
    }
  }

  const handleSitePause = async (site: Site) => {
    const toastId = toast.loading(`正在暂停 ${site.name}...`)
    try {
      await updateSiteStatus(site.id, "paused")
      toast.success(`${site.name} 已暂停`, { id: toastId })
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "无法暂停站点", { id: toastId })
    }
  }

  const handleSiteConfigure = (site: Site) => {
    console.log("配置站点:", site.name)
  }

  const handleSiteDelete = async (site: Site) => {
    if (!confirm(`确认删除站点「${site.name}」吗？此操作不可恢复。`)) {
      return
    }

    const toastId = toast.loading(`正在删除 ${site.name}...`)
    try {
      await removeSite(site.id)
      toast.success(`${site.name} 已删除`, { id: toastId })
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "删除站点失败", { id: toastId })
    }
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
              <Button variant="outline" size="sm" onClick={() => console.log("从 DbOption 导入")} className="gap-1">
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
              <EnhancedCreateSiteDialog onCreateSite={addSite} />
            </div>
          </div>

          {/* Statistics Cards */}
          <StatsPanel stats={stats} />

          {/* Filters and Search */}
          <FiltersBar
            search={filters.search}
            status={filters.status}
            environment={filters.environment}
            owner={filters.owner}
            sort={filters.sort}
            pageSize={filters.perPage}
            total={total}
            viewMode={filters.viewMode}
            onSearchChange={actions.setSearch}
            onStatusChange={actions.setStatus}
            onEnvironmentChange={actions.setEnvironment}
            onOwnerChange={actions.setOwner}
            onSortChange={actions.setSort}
            onPageSizeChange={(value) => {
              const size = parseInt(value, 10)
              actions.setPerPage(Number.isNaN(size) ? 12 : size)
            }}
            onViewModeChange={actions.setViewMode}
          />

          {/* Error Message */}
          {error && (
            <Alert variant="destructive">
              <AlertCircle className="h-4 w-4" />
              <AlertTitle>加载部署站点失败</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}

          {/* Initial Loading Skeleton */}
          {!initialized && loading ? (
            <div
              className={
                filters.viewMode === "grid"
                  ? "grid gap-4 sm:grid-cols-2 xl:grid-cols-3"
                  : "space-y-4"
              }
            >
              {Array.from({ length: filters.viewMode === "grid" ? 3 : 3 }).map((_, index) => (
                <div
                  key={index}
                  className="rounded-xl border border-border bg-muted/30 p-6 animate-pulse space-y-4"
                >
                  <div className="h-4 w-24 rounded bg-muted-foreground/30" />
                  <div className="h-6 w-1/2 rounded bg-muted-foreground/25" />
                  <div className="h-3 w-full rounded bg-muted-foreground/20" />
                  <div className="h-3 w-5/6 rounded bg-muted-foreground/20" />
                  <div className="h-4 w-2/3 rounded bg-muted-foreground/30" />
                </div>
              ))}
            </div>
          ) : (
            <>
              {/* Loading Banner */}
              {loading && (
                <Alert>
                  <RefreshCw className="h-4 w-4 animate-spin" />
                  <AlertTitle>正在刷新部署站点</AlertTitle>
                  <AlertDescription>请稍候，最新数据加载中…</AlertDescription>
                </Alert>
              )}

              {/* Site List */}
              <SiteList
                sites={sites}
                viewMode={filters.viewMode}
                onSiteView={handleSiteView}
                onSiteStart={handleSiteStart}
                onSitePause={handleSitePause}
                onSiteConfigure={handleSiteConfigure}
                onSiteDelete={handleSiteDelete}
              />
            </>
          )}

          {/* Pagination */}
          <div className="flex items-center justify-between text-sm text-muted-foreground border-t border-border pt-4">
            <span>
              第 {filters.page} / {totalPages} 页
            </span>
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                size="sm"
                disabled={filters.page <= 1 || loading}
                onClick={() => actions.setPage(Math.max(1, filters.page - 1))}
              >
                上一页
              </Button>
              <Button
                variant="outline"
                size="sm"
                disabled={filters.page >= totalPages || loading}
                onClick={() => actions.setPage(Math.min(totalPages, filters.page + 1))}
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
