"use client"

import { Inbox } from "lucide-react"
import { SiteCard, type Site } from "./site-card"

interface SiteListProps {
  sites: Site[]
  viewMode?: "grid" | "list"
  onSiteView?: (site: Site) => void
  onSiteStart?: (site: Site) => void
  onSitePause?: (site: Site) => void
  onSiteConfigure?: (site: Site) => void
  onSiteDelete?: (site: Site) => void
}

export function SiteList({
  sites,
  viewMode = "list",
  onSiteView,
  onSiteStart,
  onSitePause,
  onSiteConfigure,
  onSiteDelete,
}: SiteListProps) {
  if (sites.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center rounded-xl border border-dashed border-border bg-muted/30 px-6 py-16 text-center space-y-3">
        <Inbox className="h-10 w-10 text-muted-foreground" />
        <h3 className="text-lg font-semibold text-foreground">暂无部署站点</h3>
        <p className="text-sm text-muted-foreground">
          调整筛选条件，或点击右上角“创建站点”开始部署您的第一个环境。
        </p>
      </div>
    )
  }

  const containerClass =
    viewMode === "grid"
      ? "grid gap-4 sm:grid-cols-2 xl:grid-cols-3"
      : "space-y-4"

  return (
    <div className={containerClass}>
      {sites.map((site) => (
        <SiteCard
          key={site.id}
          site={site}
          onView={onSiteView}
          onStart={onSiteStart}
          onPause={onSitePause}
          onConfigure={onSiteConfigure}
          onDelete={onSiteDelete}
        />
      ))}
    </div>
  )
}
