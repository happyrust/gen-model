"use client"

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Progress } from "@/components/ui/progress"
import type { SystemMetrics } from "@/types/task-monitor"

interface SystemMetricsPanelProps {
  metrics: SystemMetrics
  onRefresh?: () => void
}

export function SystemMetricsPanel({ metrics, onRefresh }: SystemMetricsPanelProps) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>系统指标</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div>
          <div className="mb-2 flex justify-between text-sm"><span>CPU</span><span>{metrics.cpu}%</span></div>
          <Progress value={metrics.cpu} />
        </div>
        <div>
          <div className="mb-2 flex justify-between text-sm"><span>内存</span><span>{metrics.memory}%</span></div>
          <Progress value={metrics.memory} />
        </div>
        <div>
          <div className="mb-2 flex justify-between text-sm"><span>磁盘</span><span>{metrics.disk}%</span></div>
          <Progress value={metrics.disk} />
        </div>
        <div>
          <div className="mb-2 flex justify-between text-sm"><span>网络</span><span>{metrics.network}%</span></div>
          <Progress value={metrics.network} />
        </div>

        <div className="flex justify-end">
          <Button variant="outline" size="sm" onClick={onRefresh}>刷新</Button>
        </div>
      </CardContent>
    </Card>
  )
}


