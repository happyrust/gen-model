"use client"

import { Card, CardContent } from "@/components/ui/card"
import { Database, CheckCircle, Rocket, Settings, XCircle } from "lucide-react"

interface SiteStats {
  total: number
  running: number
  deploying: number
  configuring: number
  failed: number
  paused: number
}

interface StatsPanelProps {
  stats: SiteStats
}

export function StatsPanel({ stats }: StatsPanelProps) {
  const statItems = [
    {
      label: "监控站点",
      value: stats.total,
      icon: Database,
      color: "text-muted-foreground",
      bgColor: "bg-muted/10",
    },
    {
      label: "运行中",
      value: stats.running,
      icon: CheckCircle,
      color: "text-green-600",
      bgColor: "bg-green-50",
    },
    {
      label: "部署中",
      value: stats.deploying,
      icon: Rocket,
      color: "text-blue-600",
      bgColor: "bg-blue-50",
    },
    {
      label: "配置中",
      value: stats.configuring,
      icon: Settings,
      color: "text-orange-600",
      bgColor: "bg-orange-50",
    },
    {
      label: "失败",
      value: stats.failed,
      icon: XCircle,
      color: "text-red-600",
      bgColor: "bg-red-50",
    },
  ]

  return (
    <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-5 gap-4">
      {statItems.map((item) => {
        const Icon = item.icon
        return (
          <Card key={item.label} className="hover:shadow-md transition-shadow">
            <CardContent className="flex items-center justify-between p-4">
              <div className="space-y-1">
                <p className="text-sm text-muted-foreground">{item.label}</p>
                <p className={`text-2xl font-bold ${item.color}`}>{item.value}</p>
              </div>
              <div className={`p-2 rounded-lg ${item.bgColor}`}>
                <Icon className={`h-6 w-6 ${item.color}`} />
              </div>
            </CardContent>
          </Card>
        )
      })}
    </div>
  )
}
