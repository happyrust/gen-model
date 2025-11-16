"use client"

import { useState, useEffect } from "react"
import { Sidebar } from "@/components/sidebar"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Plus, Settings, Activity, RefreshCw, Server } from "lucide-react"
import Link from "next/link"
import { listEnvironments } from "@/lib/api/remote-sync"
import type { Environment } from "@/types/remote-sync"

export default function RemoteSyncPage() {
  const [environments, setEnvironments] = useState<Environment[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [error, setError] = useState<Error | null>(null)

  const loadEnvironments = async () => {
    setIsLoading(true)
    setError(null)
    try {
      const data = await listEnvironments()
      setEnvironments(data)
    } catch (err) {
      setError(err as Error)
    } finally {
      setIsLoading(false)
    }
  }

  useEffect(() => {
    loadEnvironments()
  }, [])

  const refetch = () => {
    loadEnvironments()
  }

  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      <main className="ml-64 p-8">
        <div className="max-w-7xl mx-auto space-y-6">
          {/* Header */}
          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-3xl font-bold tracking-tight">异地协同运维平台</h1>
              <p className="text-muted-foreground mt-2">
                管理和监控多地区数据同步环境
              </p>
            </div>
            <div className="flex items-center gap-3">
              <Button
                variant="outline"
                size="sm"
                onClick={() => refetch()}
                disabled={isLoading}
              >
                <RefreshCw className={`h-4 w-4 mr-2 ${isLoading ? 'animate-spin' : ''}`} />
                刷新
              </Button>
              <Link href="/remote-sync/deploy">
                <Button>
                  <Plus className="h-4 w-4 mr-2" />
                  部署新环境
                </Button>
              </Link>
            </div>
          </div>

          {/* Quick Actions */}
          <div className="grid gap-4 md:grid-cols-3">
            <Link href="/remote-sync/monitor">
              <Card className="hover:shadow-md transition-shadow cursor-pointer">
                <CardHeader>
                  <CardTitle className="flex items-center gap-2">
                    <Activity className="h-5 w-5" />
                    监控仪表板
                  </CardTitle>
                  <CardDescription>
                    实时查看同步状态和性能指标
                  </CardDescription>
                </CardHeader>
              </Card>
            </Link>

            <Link href="/remote-sync/flow">
              <Card className="hover:shadow-md transition-shadow cursor-pointer">
                <CardHeader>
                  <CardTitle className="flex items-center gap-2">
                    <Activity className="h-5 w-5" />
                    数据流向
                  </CardTitle>
                  <CardDescription>
                    可视化展示数据在各站点间的流向
                  </CardDescription>
                </CardHeader>
              </Card>
            </Link>

            <Link href="/remote-sync/logs">
              <Card className="hover:shadow-md transition-shadow cursor-pointer">
                <CardHeader>
                  <CardTitle className="flex items-center gap-2">
                    <Settings className="h-5 w-5" />
                    日志查询
                  </CardTitle>
                  <CardDescription>
                    查询和分析同步日志
                  </CardDescription>
                </CardHeader>
              </Card>
            </Link>
          </div>

          {/* Environment List */}
          <Card>
            <CardHeader>
              <CardTitle>环境列表</CardTitle>
              <CardDescription>
                管理所有异地协同环境
              </CardDescription>
            </CardHeader>
            <CardContent>
              {error && (
                <div className="p-4 mb-4 bg-destructive/10 text-destructive rounded-lg">
                  加载环境列表失败: {error instanceof Error ? error.message : '未知错误'}
                </div>
              )}

              {isLoading ? (
                <div className="space-y-3">
                  {Array.from({ length: 3 }).map((_, i) => (
                    <div key={i} className="h-24 bg-muted animate-pulse rounded-lg" />
                  ))}
                </div>
              ) : environments && environments.length > 0 ? (
                <div className="space-y-3">
                  {environments.map((env) => (
                    <Link key={env.id} href={`/remote-sync/${env.id}`}>
                      <div className="p-4 border rounded-lg hover:bg-accent transition-colors cursor-pointer">
                        <div className="flex items-center justify-between mb-2">
                          <div className="flex items-center gap-3">
                            <Server className="h-5 w-5 text-muted-foreground" />
                            <h3 className="font-semibold">{env.name}</h3>
                          </div>
                          <Badge variant={env.status === 'running' ? 'default' : 'secondary'}>
                            {env.status === 'running' ? '运行中' : env.status === 'paused' ? '已暂停' : '已停止'}
                          </Badge>
                        </div>
                        <div className="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm text-muted-foreground">
                          <div>
                            <span className="font-medium">位置:</span> {env.location || '未指定'}
                          </div>
                          <div>
                            <span className="font-medium">MQTT:</span> {env.mqttHost || '未配置'}
                          </div>
                          <div>
                            <span className="font-medium">站点数:</span> {env.siteCount || 0}
                          </div>
                          <div>
                            <span className="font-medium">队列:</span> {env.queueSize || 0}
                          </div>
                        </div>
                      </div>
                    </Link>
                  ))}
                </div>
              ) : (
                <div className="text-center py-12">
                  <Server className="h-12 w-12 text-muted-foreground mx-auto mb-4" />
                  <p className="text-lg font-medium mb-2">还没有环境</p>
                  <p className="text-sm text-muted-foreground mb-4">
                    创建第一个环境来开始管理异地协同
                  </p>
                  <Link href="/remote-sync/deploy">
                    <Button>
                      <Plus className="h-4 w-4 mr-2" />
                      部署新环境
                    </Button>
                  </Link>
                </div>
              )}
            </CardContent>
          </Card>
        </div>
      </main>
    </div>
  )
}
