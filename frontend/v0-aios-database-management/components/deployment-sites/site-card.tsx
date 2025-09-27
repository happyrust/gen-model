"use client"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent } from "@/components/ui/card"
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import { Database, MoreHorizontal, Play, Pause, Settings, Trash2, ExternalLink, Copy } from "lucide-react"
import { formatDistanceToNow } from "date-fns"
import { zhCN } from "date-fns/locale"

export interface Site {
  id: string
  name: string
  status: "running" | "deploying" | "configuring" | "failed" | "paused" | "stopped"
  environment: "dev" | "test" | "staging" | "prod"
  owner?: string
  createdAt: string
  updatedAt: string
  url?: string
  description?: string
}

interface SiteCardProps {
  site: Site
  onView?: (site: Site) => void
  onStart?: (site: Site) => void
  onPause?: (site: Site) => void
  onConfigure?: (site: Site) => void
  onDelete?: (site: Site) => void
}

const statusConfig = {
  running: {
    label: "运行中",
    color: "bg-green-100 text-green-800",
    icon: Play,
  },
  deploying: {
    label: "部署中",
    color: "bg-blue-100 text-blue-800",
    icon: Settings,
  },
  configuring: {
    label: "配置中",
    color: "bg-orange-100 text-orange-800",
    icon: Settings,
  },
  failed: {
    label: "失败",
    color: "bg-red-100 text-red-800",
    icon: ExternalLink,
  },
  paused: {
    label: "已暂停",
    color: "bg-gray-100 text-gray-800",
    icon: Pause,
  },
  stopped: {
    label: "已停止",
    color: "bg-gray-200 text-gray-900",
    icon: Pause,
  },
}

const environmentConfig = {
  dev: {
    label: "开发",
    color: "bg-yellow-100 text-yellow-800",
  },
  test: {
    label: "测试",
    color: "bg-purple-100 text-purple-800",
  },
  staging: {
    label: "预发布",
    color: "bg-orange-100 text-orange-800",
  },
  prod: {
    label: "生产",
    color: "bg-red-100 text-red-800",
  },
}

export function SiteCard({ site, onView, onStart, onPause, onConfigure, onDelete }: SiteCardProps) {
  const statusInfo = statusConfig[site.status]
  const envInfo = environmentConfig[site.environment] ?? environmentConfig.dev

  const handleCopyUrl = () => {
    if (site.url) {
      navigator.clipboard.writeText(site.url)
    }
  }

  return (
    <Card className="hover:shadow-md transition-shadow group">
      <CardContent className="p-6">
        <div className="flex items-start justify-between">
          <div className="flex items-start space-x-4 flex-1">
            <div className="w-12 h-12 bg-muted rounded-lg flex items-center justify-center">
              <Database className="h-6 w-6" />
            </div>

            <div className="space-y-2 flex-1">
              <div className="flex items-center space-x-2">
                <h3 className="text-lg font-semibold hover:text-primary cursor-pointer" onClick={() => onView?.(site)}>
                  {site.name}
                </h3>
                <Badge variant="secondary" className={statusInfo.color}>
                  {statusInfo.label}
                </Badge>
              </div>

              {site.description && <p className="text-sm text-muted-foreground line-clamp-2">{site.description}</p>}

              <div className="flex items-center space-x-4 text-sm text-muted-foreground">
                <Badge variant="outline" className={envInfo.color}>
                  {envInfo.label}
                </Badge>

                {site.owner && <span>负责人: {site.owner}</span>}

                <span>
                  {formatDistanceToNow(new Date(site.updatedAt), {
                    addSuffix: true,
                    locale: zhCN,
                  })}
                </span>
              </div>

              {site.url && (
                <div className="flex items-center space-x-2 text-sm">
                  <span className="text-muted-foreground">访问地址:</span>
                  <a
                    href={site.url}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-primary hover:underline flex items-center space-x-1"
                  >
                    <span>{site.url}</span>
                    <ExternalLink className="h-3 w-3" />
                  </a>
                  <Button variant="ghost" size="sm" onClick={handleCopyUrl} className="h-6 w-6 p-0">
                    <Copy className="h-3 w-3" />
                  </Button>
                </div>
              )}
            </div>
          </div>

          <div className="flex items-center space-x-2">
            <Button variant="outline" size="sm" onClick={() => onView?.(site)}>
              查看详情
            </Button>

            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="ghost" size="sm" className="h-8 w-8 p-0">
                  <MoreHorizontal className="h-4 w-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                {site.status === "paused" ? (
                  <DropdownMenuItem onClick={() => onStart?.(site)}>
                    <Play className="h-4 w-4 mr-2" />
                    启动站点
                  </DropdownMenuItem>
                ) : (
                  <DropdownMenuItem onClick={() => onPause?.(site)}>
                    <Pause className="h-4 w-4 mr-2" />
                    暂停站点
                  </DropdownMenuItem>
                )}
                <DropdownMenuItem onClick={() => onConfigure?.(site)}>
                  <Settings className="h-4 w-4 mr-2" />
                  配置站点
                </DropdownMenuItem>
                <DropdownMenuItem onClick={() => onDelete?.(site)} className="text-red-600">
                  <Trash2 className="h-4 w-4 mr-2" />
                  删除站点
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>
      </CardContent>
    </Card>
  )
}
