"use client"

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
      <div className="text-center py-12">
        <div className="text-muted-foreground">
          <p className="text-lg mb-2">暂无部署站点</p>
          <p className="text-sm">点击"创建站点"开始部署您的第一个站点</p>
        </div>
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
