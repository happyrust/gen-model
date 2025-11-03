"use client"

import { useState, useEffect, useCallback } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Progress } from "@/components/ui/progress"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { 
  Activity, 
  RefreshCw, 
  Wifi, 
  WifiOff, 
  AlertTriangle,
  CheckCircle,
  Clock,
  Play,
  Pause,
  Square
} from "lucide-react"
import { TaskStatusCard } from "./TaskStatusCard"
import { SystemMetricsPanel } from "./SystemMetricsPanel"
import { TaskQueueMonitor } from "./TaskQueueMonitor"
import { RealtimeStatusIndicator } from "./RealtimeStatusIndicator"
import { useTaskMonitor } from "@/hooks/use-task-monitor"
import { useWebSocket } from "@/hooks/use-websocket"
import type { Task, SystemMetrics, TaskQueue } from "@/types/task-monitor"

interface TaskMonitorDashboardProps {
  refreshInterval?: number
  autoRefresh?: boolean
}

export function TaskMonitorDashboard({ 
  refreshInterval = 5000, 
  autoRefresh = true 
}: TaskMonitorDashboardProps) {
  const [activeTab, setActiveTab] = useState("tasks")
  const [isManualRefresh, setIsManualRefresh] = useState(false)
  
  const {
    tasks,
    systemMetrics,
    isConnected,
    lastUpdate,
    error,
    refreshData,
    startTask,
    stopTask,
    pauseTask
  } = useTaskMonitor()

  const { isConnected: wsConnected, lastMessage } = useWebSocket('/ws/tasks/updates')

  // 自动刷新逻辑
  useEffect(() => {
    if (!autoRefresh) return

    const interval = setInterval(() => {
      if (!isManualRefresh) {
        refreshData()
      }
    }, refreshInterval)

    return () => clearInterval(interval)
  }, [autoRefresh, refreshInterval, isManualRefresh, refreshData])

  // WebSocket消息处理
  useEffect(() => {
    if (lastMessage) {
      handleWebSocketMessage(lastMessage)
    }
  }, [lastMessage])

  const handleWebSocketMessage = useCallback((message: any) => {
    // 处理实时更新消息
    if (message.type === 'task_update') {
      // 更新任务状态
      refreshData()
    } else if (message.type === 'system_metrics') {
      // 更新系统指标
      refreshData()
    }
  }, [refreshData])

  const handleManualRefresh = useCallback(async () => {
    setIsManualRefresh(true)
    await refreshData()
    setTimeout(() => setIsManualRefresh(false), 1000)
  }, [refreshData])

  const handleTaskAction = useCallback(async (taskId: string, action: string) => {
    try {
      switch (action) {
        case 'start':
          await startTask(taskId)
          break
        case 'stop':
          await stopTask(taskId)
          break
        case 'pause':
          await pauseTask(taskId)
          break
      }
    } catch (error) {
      console.error('Task action failed:', error)
    }
  }, [startTask, stopTask, pauseTask])

  const runningTasks = tasks.filter(task => task.status === 'running')
  const pendingTasks = tasks.filter(task => task.status === 'pending')
  const completedTasks = tasks.filter(task => task.status === 'completed')
  const failedTasks = tasks.filter(task => task.status === 'failed')

  return (
    <div className="space-y-6">
      {/* 头部状态栏 */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-4">
          <h2 className="text-2xl font-bold">任务监控</h2>
          <RealtimeStatusIndicator 
            isConnected={wsConnected}
            lastUpdate={lastUpdate}
            onReconnect={handleManualRefresh}
          />
        </div>
        
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={handleManualRefresh}
            disabled={isManualRefresh}
          >
            <RefreshCw className={`h-4 w-4 mr-2 ${isManualRefresh ? 'animate-spin' : ''}`} />
            刷新
          </Button>
        </div>
      </div>

      {/* 错误提示 */}
      {error && (
        <Card className="border-red-200 bg-red-50">
          <CardContent className="p-4">
            <div className="flex items-center gap-2 text-red-600">
              <AlertTriangle className="h-4 w-4" />
              <span>{error}</span>
            </div>
          </CardContent>
        </Card>
      )}

      {/* 任务概览卡片 */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-2">
              <Play className="h-4 w-4 text-green-600" />
              <span className="text-sm font-medium">运行中</span>
            </div>
            <div className="text-2xl font-bold text-green-600">{runningTasks.length}</div>
          </CardContent>
        </Card>
        
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-2">
              <Clock className="h-4 w-4 text-yellow-600" />
              <span className="text-sm font-medium">等待中</span>
            </div>
            <div className="text-2xl font-bold text-yellow-600">{pendingTasks.length}</div>
          </CardContent>
        </Card>
        
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-2">
              <CheckCircle className="h-4 w-4 text-blue-600" />
              <span className="text-sm font-medium">已完成</span>
            </div>
            <div className="text-2xl font-bold text-blue-600">{completedTasks.length}</div>
          </CardContent>
        </Card>
        
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-2">
              <AlertTriangle className="h-4 w-4 text-red-600" />
              <span className="text-sm font-medium">失败</span>
            </div>
            <div className="text-2xl font-bold text-red-600">{failedTasks.length}</div>
          </CardContent>
        </Card>
      </div>

      {/* 主要内容区域 */}
      <Tabs value={activeTab} onValueChange={setActiveTab}>
        <TabsList className="grid w-full grid-cols-3">
          <TabsTrigger value="tasks">任务状态</TabsTrigger>
          <TabsTrigger value="system">系统监控</TabsTrigger>
          <TabsTrigger value="queue">任务队列</TabsTrigger>
        </TabsList>

        <TabsContent value="tasks" className="space-y-4">
          <div className="grid gap-4">
            {tasks.map((task) => (
              <TaskStatusCard
                key={task.id}
                task={task}
                onTaskAction={handleTaskAction}
              />
            ))}
            {tasks.length === 0 && (
              <Card>
                <CardContent className="p-8 text-center text-muted-foreground">
                  暂无任务数据
                </CardContent>
              </Card>
            )}
          </div>
        </TabsContent>

        <TabsContent value="system">
          <SystemMetricsPanel 
            metrics={systemMetrics}
            onRefresh={handleManualRefresh}
          />
        </TabsContent>

        <TabsContent value="queue">
          <TaskQueueMonitor 
            queue={{ pending: pendingTasks, running: runningTasks }}
            onQueueAction={(action) => {
              console.log('Queue action:', action)
            }}
          />
        </TabsContent>
      </Tabs>
    </div>
  )
}
