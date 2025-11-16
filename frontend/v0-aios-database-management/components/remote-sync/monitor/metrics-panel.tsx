"use client"

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Activity, Database, Zap, TrendingUp } from "lucide-react"
import type { Metrics } from "@/types/remote-sync"

interface MetricsPanelProps {
  metrics: Metrics
}

export function MetricsPanel({ metrics }: MetricsPanelProps) {
  const formatRate = (mbps: number) => {
    if (mbps < 1) return `${(mbps * 1024).toFixed(1)} KB/s`
    return `${mbps.toFixed(1)} MB/s`
  }

  const formatTime = (ms: number) => {
    if (ms < 1000) return `${ms} ms`
    return `${(ms / 1000).toFixed(1)} s`
  }

  const getSuccessRate = () => {
    const total = metrics.totalSynced + metrics.totalFailed
    if (total === 0) return 0
    return ((metrics.totalSynced / total) * 100).toFixed(1)
  }

  return (
    <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
      {/* 同步速率 */}
      <Card>
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-sm font-medium">同步速率</CardTitle>
          <Zap className="h-4 w-4 text-muted-foreground" />
        </CardHeader>
        <CardContent>
          <div className="text-2xl font-bold">{formatRate(metrics.syncRate)}</div>
          <p className="text-xs text-muted-foreground mt-1">
            平均: {formatTime(metrics.avgSyncTime)}
          </p>
        </CardContent>
      </Card>

      {/* 队列长度 */}
      <Card>
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-sm font-medium">队列长度</CardTitle>
          <Database className="h-4 w-4 text-muted-foreground" />
        </CardHeader>
        <CardContent>
          <div className="text-2xl font-bold">{metrics.queueLength}</div>
          <p className="text-xs text-muted-foreground mt-1">
            活跃: {metrics.activeTasks} 个任务
          </p>
        </CardContent>
      </Card>

      {/* 成功率 */}
      <Card>
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-sm font-medium">成功率</CardTitle>
          <TrendingUp className="h-4 w-4 text-muted-foreground" />
        </CardHeader>
        <CardContent>
          <div className="text-2xl font-bold">{getSuccessRate()}%</div>
          <p className="text-xs text-muted-foreground mt-1">
            成功: {metrics.totalSynced} / 失败: {metrics.totalFailed}
          </p>
        </CardContent>
      </Card>

      {/* 系统资源 */}
      <Card>
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-sm font-medium">系统资源</CardTitle>
          <Activity className="h-4 w-4 text-muted-foreground" />
        </CardHeader>
        <CardContent>
          <div className="text-2xl font-bold">{metrics.cpuUsage.toFixed(1)}%</div>
          <p className="text-xs text-muted-foreground mt-1">
            内存: {metrics.memoryUsage.toFixed(1)}%
          </p>
        </CardContent>
      </Card>
    </div>
  )
}
