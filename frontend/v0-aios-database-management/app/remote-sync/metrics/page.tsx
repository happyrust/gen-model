'use client'

import { useState, useEffect } from 'react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { 
  LineChart, 
  Line, 
  AreaChart,
  Area,
  XAxis, 
  YAxis, 
  CartesianGrid, 
  Tooltip, 
  Legend, 
  ResponsiveContainer 
} from 'recharts'
import { 
  Activity, 
  Database, 
  Zap, 
  TrendingUp, 
  Clock,
  Download,
  RefreshCw,
} from 'lucide-react'
import { useToast } from '@/hooks/use-toast'

interface MetricsData {
  syncRate: number
  avgSyncTime: number
  totalSynced: number
  totalFailed: number
  queueLength: number
  activeTasks: number
  cpuUsage: number
  memoryUsage: number
  successRate: number
  completedFiles: number
  completedBytes: number
  completedRecords: number
  failedFiles: number
}

interface HistoryPoint {
  timestamp: string
  task_count: number
  completed_count: number
  failed_count: number
  total_bytes: number
  avg_sync_time_ms: number
}

export default function MetricsPage() {
  const [metrics, setMetrics] = useState<MetricsData | null>(null)
  const [history, setHistory] = useState<HistoryPoint[]>([])
  const [timeRange, setTimeRange] = useState<'hour' | 'day' | 'week' | 'month'>('day')
  const [isLoading, setIsLoading] = useState(false)
  const { toast } = useToast()

  // 加载当前指标
  const loadMetrics = async () => {
    try {
      const response = await fetch('/api/sync/metrics')
      if (!response.ok) throw new Error('加载指标失败')
      
      const result = await response.json()
      if (result.status === 'success' && result.metrics) {
        const m = result.metrics
        setMetrics({
          syncRate: m.sync_rate_mbps || 0,
          avgSyncTime: m.avg_sync_time_ms || 0,
          totalSynced: m.total_synced || 0,
          totalFailed: m.total_failed || 0,
          queueLength: 0,
          activeTasks: 0,
          cpuUsage: m.cpu_usage || 0,
          memoryUsage: m.memory_usage || 0,
          successRate: m.success_rate || 0,
          completedFiles: m.completed_files_total || 0,
          completedBytes: m.completed_bytes_total || 0,
          completedRecords: m.completed_records_total || 0,
          failedFiles: m.failed_files_total || 0,
        })
      }
    } catch (error) {
      console.error('加载指标失败:', error)
    }
  }

  // 加载历史数据
  const loadHistory = async () => {
    setIsLoading(true)
    try {
      const response = await fetch(`/api/sync/metrics/history?time_range=${timeRange}&limit=100`)
      if (!response.ok) throw new Error('加载历史数据失败')
      
      const result = await response.json()
      if (result.status === 'success' && result.history) {
        setHistory(result.history)
      }
    } catch (error) {
      toast({
        title: '加载失败',
        description: error instanceof Error ? error.message : '未知错误',
        variant: 'destructive',
      })
    } finally {
      setIsLoading(false)
    }
  }

  // 导出报告
  const exportReport = () => {
    if (!metrics || history.length === 0) {
      toast({
        title: '导出失败',
        description: '没有可导出的数据',
        variant: 'destructive',
      })
      return
    }

    const csvContent = [
      ['时间', '任务数', '完成数', '失败数', '总字节数', '平均耗时(ms)'],
      ...history.map(h => [
        h.timestamp,
        h.task_count,
        h.completed_count,
        h.failed_count,
        h.total_bytes,
        h.avg_sync_time_ms.toFixed(2),
      ])
    ].map(row => row.join(',')).join('\n')

    const blob = new Blob([csvContent], { type: 'text/csv;charset=utf-8;' })
    const link = document.createElement('a')
    link.href = URL.createObjectURL(blob)
    link.download = `metrics-${timeRange}-${new Date().toISOString()}.csv`
    link.click()

    toast({
      title: '导出成功',
      description: '性能报告已下载',
    })
  }

  useEffect(() => {
    loadMetrics()
    loadHistory()
    
    const interval = setInterval(() => {
      loadMetrics()
    }, 5000)
    
    return () => clearInterval(interval)
  }, [timeRange])

  const formatBytes = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`
  }

  const formatTime = (ms: number) => {
    if (ms < 1000) return `${ms.toFixed(0)} ms`
    return `${(ms / 1000).toFixed(1)} s`
  }

  return (
    <div className="min-h-screen bg-background p-8">
      <div className="max-w-7xl mx-auto space-y-6">
        {/* Header */}
        <div className="flex items-center justify-between">
          <div>
            <h1 className="text-3xl font-bold tracking-tight">性能监控</h1>
            <p className="text-muted-foreground mt-1">
              实时性能指标和历史趋势分析
            </p>
          </div>
          <div className="flex items-center gap-2">
            <Button onClick={loadHistory} variant="outline" size="sm" disabled={isLoading}>
              <RefreshCw className="w-4 h-4 mr-2" />
              刷新
            </Button>
            <Button onClick={exportReport} variant="outline" size="sm">
              <Download className="w-4 h-4 mr-2" />
              导出报告
            </Button>
          </div>
        </div>

        {/* 实时指标卡片 */}
        {metrics && (
          <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">同步速率</CardTitle>
                <Zap className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">
                  {metrics.syncRate < 1 
                    ? `${(metrics.syncRate * 1024).toFixed(1)} KB/s`
                    : `${metrics.syncRate.toFixed(1)} MB/s`
                  }
                </div>
                <p className="text-xs text-muted-foreground mt-1">
                  平均耗时: {formatTime(metrics.avgSyncTime)}
                </p>
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">成功率</CardTitle>
                <TrendingUp className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{metrics.successRate.toFixed(1)}%</div>
                <p className="text-xs text-muted-foreground mt-1">
                  成功: {metrics.totalSynced} / 失败: {metrics.totalFailed}
                </p>
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">完成统计</CardTitle>
                <Database className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{metrics.completedFiles}</div>
                <p className="text-xs text-muted-foreground mt-1">
                  {formatBytes(metrics.completedBytes)} / {metrics.completedRecords} 条记录
                </p>
              </CardContent>
            </Card>

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
        )}

        {/* 历史趋势图 */}
        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <CardTitle>历史趋势</CardTitle>
              <Tabs value={timeRange} onValueChange={(v) => setTimeRange(v as any)}>
                <TabsList>
                  <TabsTrigger value="hour">1小时</TabsTrigger>
                  <TabsTrigger value="day">24小时</TabsTrigger>
                  <TabsTrigger value="week">7天</TabsTrigger>
                  <TabsTrigger value="month">30天</TabsTrigger>
                </TabsList>
              </Tabs>
            </div>
          </CardHeader>
          <CardContent>
            <Tabs defaultValue="tasks">
              <TabsList className="mb-4">
                <TabsTrigger value="tasks">任务统计</TabsTrigger>
                <TabsTrigger value="bytes">数据量</TabsTrigger>
                <TabsTrigger value="time">耗时分析</TabsTrigger>
              </TabsList>

              <TabsContent value="tasks">
                <ResponsiveContainer width="100%" height={300}>
                  <AreaChart data={history}>
                    <CartesianGrid strokeDasharray="3 3" />
                    <XAxis 
                      dataKey="timestamp" 
                      tickFormatter={(value) => new Date(value).toLocaleTimeString()}
                    />
                    <YAxis />
                    <Tooltip 
                      labelFormatter={(value) => new Date(value).toLocaleString()}
                    />
                    <Legend />
                    <Area 
                      type="monotone" 
                      dataKey="completed_count" 
                      stackId="1"
                      stroke="#10b981" 
                      fill="#10b981" 
                      name="完成"
                    />
                    <Area 
                      type="monotone" 
                      dataKey="failed_count" 
                      stackId="1"
                      stroke="#ef4444" 
                      fill="#ef4444" 
                      name="失败"
                    />
                  </AreaChart>
                </ResponsiveContainer>
              </TabsContent>

              <TabsContent value="bytes">
                <ResponsiveContainer width="100%" height={300}>
                  <LineChart data={history}>
                    <CartesianGrid strokeDasharray="3 3" />
                    <XAxis 
                      dataKey="timestamp" 
                      tickFormatter={(value) => new Date(value).toLocaleTimeString()}
                    />
                    <YAxis tickFormatter={(value) => formatBytes(value)} />
                    <Tooltip 
                      labelFormatter={(value) => new Date(value).toLocaleString()}
                      formatter={(value: any) => formatBytes(value)}
                    />
                    <Legend />
                    <Line 
                      type="monotone" 
                      dataKey="total_bytes" 
                      stroke="#3b82f6" 
                      name="传输数据量"
                      strokeWidth={2}
                    />
                  </LineChart>
                </ResponsiveContainer>
              </TabsContent>

              <TabsContent value="time">
                <ResponsiveContainer width="100%" height={300}>
                  <LineChart data={history}>
                    <CartesianGrid strokeDasharray="3 3" />
                    <XAxis 
                      dataKey="timestamp" 
                      tickFormatter={(value) => new Date(value).toLocaleTimeString()}
                    />
                    <YAxis tickFormatter={(value) => formatTime(value)} />
                    <Tooltip 
                      labelFormatter={(value) => new Date(value).toLocaleString()}
                      formatter={(value: any) => formatTime(value)}
                    />
                    <Legend />
                    <Line 
                      type="monotone" 
                      dataKey="avg_sync_time_ms" 
                      stroke="#8b5cf6" 
                      name="平均耗时"
                      strokeWidth={2}
                    />
                  </LineChart>
                </ResponsiveContainer>
              </TabsContent>
            </Tabs>
          </CardContent>
        </Card>

        {/* 统计面板 */}
        {history.length > 0 && (
          <Card>
            <CardHeader>
              <CardTitle>统计分析</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="grid gap-4 md:grid-cols-3">
                <div>
                  <div className="text-sm text-muted-foreground">P50 (中位数)</div>
                  <div className="text-2xl font-bold">
                    {formatTime(
                      history
                        .map(h => h.avg_sync_time_ms)
                        .sort((a, b) => a - b)[Math.floor(history.length * 0.5)]
                    )}
                  </div>
                </div>
                <div>
                  <div className="text-sm text-muted-foreground">P95</div>
                  <div className="text-2xl font-bold">
                    {formatTime(
                      history
                        .map(h => h.avg_sync_time_ms)
                        .sort((a, b) => a - b)[Math.floor(history.length * 0.95)]
                    )}
                  </div>
                </div>
                <div>
                  <div className="text-sm text-muted-foreground">P99</div>
                  <div className="text-2xl font-bold">
                    {formatTime(
                      history
                        .map(h => h.avg_sync_time_ms)
                        .sort((a, b) => a - b)[Math.floor(history.length * 0.99)]
                    )}
                  </div>
                </div>
              </div>
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  )
}
