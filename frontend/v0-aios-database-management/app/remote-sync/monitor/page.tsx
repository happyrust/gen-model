"use client"

import { useState, useEffect } from "react"
import { Sidebar } from "@/components/sidebar"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { RefreshCw } from "lucide-react"
import { useSSE } from "@/hooks/use-sse"
import { listEnvironments, getMetrics, getTaskQueue } from "@/lib/api/remote-sync"
import { EnvironmentCard } from "@/components/remote-sync/monitor/environment-card"
import { TaskList } from "@/components/remote-sync/monitor/task-list"
import { MetricsPanel } from "@/components/remote-sync/monitor/metrics-panel"
import { AlertBanner } from "@/components/remote-sync/monitor/alert-banner"
import { useAlerts } from "@/contexts/alert-context"
import type { SyncEvent, Environment, Metrics, SyncTask } from "@/types/remote-sync"

export default function MonitorPage() {
  const [connectionStatus, setConnectionStatus] = useState<'connecting' | 'connected' | 'disconnected'>('connecting')
  const { addAlert } = useAlerts()

  // 查询数据
  const [environments, setEnvironments] = useState<Environment[]>([])
  const [metrics, setMetrics] = useState<Metrics | null>(null)
  const [tasks, setTasks] = useState<SyncTask[]>([])

  const loadData = async () => {
    try {
      const [envs, metricsData, tasksData] = await Promise.all([
        listEnvironments(),
        getMetrics().catch(() => null),
        getTaskQueue().catch(() => []),
      ])
      setEnvironments(envs)
      setMetrics(metricsData)
      setTasks(tasksData)
    } catch (err) {
      console.error('Failed to load data:', err)
    }
  }

  useEffect(() => {
    loadData()
    const interval = setInterval(loadData, 5000) // 每 5 秒刷新
    return () => clearInterval(interval)
  }, [])

  const refetchEnvs = () => {
    loadData()
  }

  // SSE 连接
  const { connected, error, reconnecting, reconnectAttempts } = useSSE({
    url: `${process.env.NEXT_PUBLIC_API_BASE_URL || 'http://localhost:8080'}/api/sync/events`,
    onMessage: (event: SyncEvent) => {
      handleSyncEvent(event)
    },
    onOpen: () => {
      setConnectionStatus('connected')
    },
    onError: (err) => {
      console.error('SSE Error:', err)
      setConnectionStatus('disconnected')
    },
    enabled: true,
  })

  useEffect(() => {
    if (connected) {
      setConnectionStatus('connected')
    } else if (reconnecting) {
      setConnectionStatus('connecting')
    } else {
      setConnectionStatus('disconnected')
    }
  }, [connected, reconnecting])

  // 处理同步事件
  const handleSyncEvent = (event: SyncEvent) => {
    switch (event.type) {
      case 'SyncFailed':
        addAlert({
          type: 'error',
          title: '同步失败',
          message: `文件 ${event.data.file_path} 同步失败: ${event.data.error}`,
          actionUrl: '/remote-sync/logs',
        })
        break
      case 'MqttDisconnected':
        addAlert({
          type: 'warning',
          title: 'MQTT 连接断开',
          message: `环境 ${event.data.env_id} 的 MQTT 连接已断开: ${event.data.reason}`,
        })
        break
      case 'QueueSizeChanged':
        if (event.data.queue_size && event.data.queue_size > 50) {
          addAlert({
            type: 'warning',
            title: '队列积压',
            message: `当前队列长度: ${event.data.queue_size}，建议检查同步状态`,
          })
        }
        break
    }
  }

  const getStatusBadge = () => {
    switch (connectionStatus) {
      case 'connected':
        return <Badge variant="default" className="bg-green-500">实时连接</Badge>
      case 'connecting':
        return <Badge variant="secondary">连接中...</Badge>
      case 'disconnected':
        return <Badge variant="destructive">连接断开</Badge>
    }
  }

  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      <main className="ml-64 p-8">
        <div className="max-w-7xl mx-auto space-y-6">
          {/* Header */}
          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-3xl font-bold tracking-tight">监控仪表板</h1>
              <p className="text-muted-foreground mt-1">
                实时监控同步状态和性能指标
              </p>
            </div>
            <div className="flex items-center gap-3">
              {getStatusBadge()}
              <Button
                variant="outline"
                size="sm"
                onClick={() => refetchEnvs()}
              >
                <RefreshCw className="h-4 w-4 mr-2" />
                刷新
              </Button>
            </div>
          </div>

          {/* 告警横幅 */}
          <AlertBanner />

          {/* 错误提示 */}
          {error && (
            <div className="p-4 bg-destructive/10 text-destructive rounded-lg">
              SSE 连接错误: {error}
              {reconnecting && ` (重连尝试: ${reconnectAttempts})`}
            </div>
          )}

          {/* 性能指标 */}
          {metrics && <MetricsPanel metrics={metrics} />}

          {/* 环境状态卡片 */}
          {environments && environments.length > 0 && (
            <div>
              <h2 className="text-lg font-semibold mb-4">环境状态</h2>
              <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
                {environments.map((env: any) => (
                  <EnvironmentCard
                    key={env.id}
                    environment={env}
                    onClick={() => window.location.href = `/remote-sync/${env.id}`}
                  />
                ))}
              </div>
            </div>
          )}

          {/* 任务列表 */}
          {tasks && <TaskList tasks={tasks} />}
        </div>
      </main>
    </div>
  )
}
