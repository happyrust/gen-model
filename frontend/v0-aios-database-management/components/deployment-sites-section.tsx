"use client"

import { useMemo, useState } from "react"

import { useDeploymentSites } from "@/hooks/use-deployment-sites"
import { DeploymentSiteCreateDialog } from "@/components/deployment-site-create-dialog"
import { DeploymentSiteDrawer } from "@/components/deployment-site-drawer"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"

interface StatusCard {
  key: string
  label: string
  value: number
  accentClass: string
}

export function DeploymentSitesSection() {
  const { items, filters, setFilter, setPage, pagination, stats, loading, error, refresh } =
    useDeploymentSites({ per_page: 6 })

  const [selectedSiteId, setSelectedSiteId] = useState<string | null>(null)
  const [drawerOpen, setDrawerOpen] = useState(false)

  const statusCards: StatusCard[] = useMemo(
    () => [
      { key: "total", label: "监控站点", value: stats.total, accentClass: "text-slate-600" },
      { key: "running", label: "运行中", value: stats.running, accentClass: "text-green-600" },
      { key: "deploying", label: "部署中", value: stats.deploying, accentClass: "text-blue-600" },
      { key: "configuring", label: "配置中", value: stats.configuring, accentClass: "text-amber-600" },
      { key: "failed", label: "失败", value: stats.failed, accentClass: "text-red-600" },
    ],
    [stats],
  )

  const openDetail = (id: string) => {
    setSelectedSiteId(id)
    setDrawerOpen(true)
  }

  return (
    <Card className="mb-8">
      <CardHeader className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex items-center gap-3">
          <div className="rounded-lg bg-primary/10 p-2 text-primary">站</div>
          <div>
            <CardTitle className="text-xl">部署站点</CardTitle>
            <p className="text-xs text-muted-foreground">管理站点状态、健康检查与任务入口</p>
          </div>
        </div>
        <div className="flex gap-3">
          <Button variant="outline" onClick={() => refresh()} disabled={loading}>
            {loading ? "刷新中..." : "刷新"}
          </Button>
          <DeploymentSiteCreateDialog onCreated={refresh} />
        </div>
      </CardHeader>
      <CardContent className="space-y-6">
        <div className="grid grid-cols-1 gap-3 md:grid-cols-5">
          {statusCards.map((card) => (
            <div key={card.key} className="rounded-lg border border-border/60 bg-muted/20 p-4">
              <p className="text-xs text-muted-foreground">{card.label}</p>
              <p className={`mt-1 text-2xl font-semibold ${card.accentClass}`}>{card.value}</p>
            </div>
          ))}
        </div>

        <div className="flex flex-wrap items-center gap-3">
          <Input
            value={filters.q ?? ""}
            onChange={(event) => setFilter({ q: event.target.value })}
            placeholder="搜索名称/描述/负责人"
            className="max-w-xs"
          />
          <select
            value={filters.status ?? ""}
            onChange={(event) => setFilter({ status: event.target.value })}
            className="h-9 rounded-md border border-input bg-background px-2 text-sm"
          >
            <option value="">全部状态</option>
            <option value="Running">运行中</option>
            <option value="Deploying">部署中</option>
            <option value="Configuring">配置中</option>
            <option value="Failed">失败</option>
          </select>
          <select
            value={filters.env ?? ""}
            onChange={(event) => setFilter({ env: event.target.value })}
            className="h-9 rounded-md border border-input bg-background px-2 text-sm"
          >
            <option value="">全部环境</option>
            <option value="dev">dev</option>
            <option value="staging">staging</option>
            <option value="prod">prod</option>
            <option value="test">test</option>
          </select>
        </div>

        {error && <p className="text-sm text-destructive">{error}</p>}

        <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
          {loading && items.length === 0 ? (
            <p className="col-span-full text-sm text-muted-foreground">正在加载部署站点...</p>
          ) : items.length === 0 ? (
            <p className="col-span-full text-sm text-muted-foreground">暂无部署站点</p>
          ) : (
            items.map((site) => (
              <div
                key={site.id}
                className="cursor-pointer rounded-lg border border-border hover:border-primary/40"
                onClick={() => openDetail(site.id)}
              >
                <div className="flex items-start justify-between border-b border-border/60 px-4 py-3">
                  <div>
                    <p className="text-base font-semibold text-foreground">{site.name ?? site.id}</p>
                    <p className="text-xs text-muted-foreground line-clamp-2">
                      {site.description ?? "暂无描述"}
                    </p>
                  </div>
                  {site.status ? (
                    <Badge className="bg-primary/10 text-primary">{site.status}</Badge>
                  ) : null}
                </div>
                <div className="space-y-2 px-4 py-3 text-xs text-muted-foreground">
                  <div className="flex items-center gap-2">
                    <span>环境：</span>
                    <span className="font-medium text-foreground">{site.env ?? "--"}</span>
                  </div>
                  <div className="flex items-center gap-2">
                    <span>负责人：</span>
                    <span className="font-medium text-foreground">{site.owner ?? "--"}</span>
                  </div>
                  {site.last_health_check && (
                    <div className="flex items-center gap-2">
                      <span>最近检查：</span>
                      <span className="font-medium text-foreground">{site.last_health_check}</span>
                    </div>
                  )}
                </div>
                <div className="flex gap-2 border-t border-border/60 px-4 py-3">
                  <Button
                    variant="outline"
                    className="flex-1"
                    onClick={(event) => {
                      event.stopPropagation()
                      openDetail(site.id)
                    }}
                  >
                    详情
                  </Button>
                  <Button
                    variant="outline"
                    className="flex-1"
                    onClick={(event) => {
                      event.stopPropagation()
                      setSelectedSiteId(site.id)
                      setDrawerOpen(true)
                    }}
                  >
                    操作
                  </Button>
                </div>
              </div>
            ))
          )}
        </div>

        <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
          <div className="text-xs text-muted-foreground">
            共 {pagination.total} 条记录，第 {pagination.page} / {pagination.pages} 页
          </div>
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              onClick={() => setPage(Math.max(1, (pagination.page ?? 1) - 1))}
              disabled={loading || pagination.page <= 1}
            >
              上一页
            </Button>
            <Button
              variant="outline"
              onClick={() => setPage(Math.min(pagination.pages, (pagination.page ?? 1) + 1))}
              disabled={loading || pagination.page >= pagination.pages}
            >
              下一页
            </Button>
          </div>
        </div>
      </CardContent>

      <DeploymentSiteDrawer
        siteId={selectedSiteId}
        open={drawerOpen}
        onOpenChange={(open) => {
          setDrawerOpen(open)
          if (!open) {
            setSelectedSiteId(null)
          }
        }}
      />
    </Card>
  )
}
