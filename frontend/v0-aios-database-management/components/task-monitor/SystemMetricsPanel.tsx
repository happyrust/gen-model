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
  const cpuValue = metrics.cpu ?? 0
  const memoryValue = metrics.memory ?? 0
  const diskValue = metrics.disk ?? 0
  const networkValue = metrics.network ?? 0

  return (
    <Card>
      <CardHeader>
        <CardTitle>系统指标</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div>
          <div className="mb-2 flex justify-between text-sm"><span>CPU</span><span>{cpuValue}%</span></div>
          <Progress value={cpuValue} />
        </div>
        <div>
          <div className="mb-2 flex justify-between text-sm"><span>内存</span><span>{memoryValue}%</span></div>
          <Progress value={memoryValue} />
        </div>
        <div>
          <div className="mb-2 flex justify-between text-sm"><span>磁盘</span><span>{diskValue}%</span></div>
          <Progress value={diskValue} />
        </div>
        <div>
          <div className="mb-2 flex justify-between text-sm"><span>网络</span><span>{networkValue}%</span></div>
          <Progress value={networkValue} />
        </div>

        <div className="flex justify-end">
          <Button variant="outline" size="sm" onClick={onRefresh}>刷新</Button>
        </div>
      </CardContent>
    </Card>
  )
}

