"use client"

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Server, Activity, Database, Wifi, WifiOff } from "lucide-react"
import type { Environment } from "@/types/remote-sync"

interface EnvironmentCardProps {
  environment: Environment
  onClick?: () => void
}

export function EnvironmentCard({ environment, onClick }: EnvironmentCardProps) {
  const getStatusColor = (status?: string) => {
    switch (status) {
      case 'running':
        return 'bg-green-500'
      case 'paused':
        return 'bg-yellow-500'
      case 'stopped':
        return 'bg-gray-500'
      default:
        return 'bg-gray-400'
    }
  }

  const getStatusText = (status?: string) => {
    switch (status) {
      case 'running':
        return '运行中'
      case 'paused':
        return '已暂停'
      case 'stopped':
        return '已停止'
      default:
        return '未知'
    }
  }

  return (
    <Card
      className="hover:shadow-md transition-shadow cursor-pointer"
      onClick={onClick}
    >
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Server className="h-5 w-5 text-muted-foreground" />
            <CardTitle className="text-lg">{environment.name}</CardTitle>
          </div>
          <Badge variant={environment.status === 'running' ? 'default' : 'secondary'}>
            <div className={`w-2 h-2 rounded-full mr-2 ${getStatusColor(environment.status)}`} />
            {getStatusText(environment.status)}
          </Badge>
        </div>
      </CardHeader>
      <CardContent>
        <div className="grid grid-cols-2 gap-4">
          {/* MQTT 状态 */}
          <div className="flex items-center gap-2">
            {environment.mqttConnected ? (
              <Wifi className="h-4 w-4 text-green-500" />
            ) : (
              <WifiOff className="h-4 w-4 text-gray-400" />
            )}
            <div>
              <div className="text-xs text-muted-foreground">MQTT</div>
              <div className="text-sm font-medium">
                {environment.mqttConnected ? '已连接' : '未连接'}
              </div>
            </div>
          </div>

          {/* 文件监控状态 */}
          <div className="flex items-center gap-2">
            <Activity className={`h-4 w-4 ${environment.watcherActive ? 'text-green-500' : 'text-gray-400'}`} />
            <div>
              <div className="text-xs text-muted-foreground">监控</div>
              <div className="text-sm font-medium">
                {environment.watcherActive ? '活跃' : '未活跃'}
              </div>
            </div>
          </div>

          {/* 站点数量 */}
          <div className="flex items-center gap-2">
            <Server className="h-4 w-4 text-muted-foreground" />
            <div>
              <div className="text-xs text-muted-foreground">站点</div>
              <div className="text-sm font-medium">{environment.siteCount || 0} 个</div>
            </div>
          </div>

          {/* 队列大小 */}
          <div className="flex items-center gap-2">
            <Database className="h-4 w-4 text-muted-foreground" />
            <div>
              <div className="text-xs text-muted-foreground">队列</div>
              <div className="text-sm font-medium">{environment.queueSize || 0} 个</div>
            </div>
          </div>
        </div>

        {/* 位置信息 */}
        {environment.location && (
          <div className="mt-3 pt-3 border-t">
            <div className="text-xs text-muted-foreground">位置</div>
            <div className="text-sm">{environment.location}</div>
          </div>
        )}
      </CardContent>
    </Card>
  )
}
