"use client"

import { useState, useEffect } from "react"
import { useRouter } from "next/navigation"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { 
  Plus, 
  Search, 
  Filter, 
  RefreshCw, 
  Play, 
  Pause, 
  Square,
  MoreHorizontal,
  Eye,
  Download
} from "lucide-react"
import { TaskMonitorDashboard } from "@/components/task-monitor/TaskMonitorDashboard"
import { Sidebar } from "@/components/sidebar"
import { useTaskMonitor } from "@/hooks/use-task-monitor"

export default function TaskMonitorPage() {
  const router = useRouter()
  const [searchTerm, setSearchTerm] = useState("")
  const [statusFilter, setStatusFilter] = useState<string>("all")
  const [typeFilter, setTypeFilter] = useState<string>("all")
  const [activeTab, setActiveTab] = useState("dashboard")

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

  // 过滤任务
  const filteredTasks = tasks.filter(task => {
    const matchesSearch = task.name.toLowerCase().includes(searchTerm.toLowerCase())
    const matchesStatus = statusFilter === "all" || task.status === statusFilter
    const matchesType = typeFilter === "all" || task.type === typeFilter
    return matchesSearch && matchesStatus && matchesType
  })

  const handleCreateTask = () => {
    router.push('/task-creation')
  }

  const handleTaskAction = async (taskId: string, action: string) => {
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
  }

  const getStatusColor = (status: string) => {
    switch (status) {
      case 'running': return 'bg-green-100 text-green-800'
      case 'pending': return 'bg-yellow-100 text-yellow-800'
      case 'completed': return 'bg-blue-100 text-blue-800'
      case 'failed': return 'bg-red-100 text-red-800'
      case 'paused': return 'bg-orange-100 text-orange-800'
      case 'cancelled': return 'bg-gray-100 text-gray-800'
      default: return 'bg-gray-100 text-gray-800'
    }
  }

  const getStatusText = (status: string) => {
    switch (status) {
      case 'running': return '运行中'
      case 'pending': return '等待中'
      case 'completed': return '已完成'
      case 'failed': return '失败'
      case 'paused': return '已暂停'
      case 'cancelled': return '已取消'
      default: return status
    }
  }

  const getTypeText = (type: string) => {
    switch (type) {
      case 'DataParsingWizard': return '数据解析'
      case 'ModelGeneration': return '模型生成'
      case 'SpatialTreeGeneration': return '空间树生成'
      case 'FullSync': return '全量同步'
      case 'IncrementalSync': return '增量同步'
      default: return type
    }
  }

  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      
      <div className="ml-64 p-8">
        <div className="max-w-7xl mx-auto">
          {/* 页面头部 */}
          <div className="flex items-center justify-between mb-8">
            <div>
              <h1 className="text-3xl font-bold text-foreground">任务监控</h1>
              <p className="text-muted-foreground">管理和监控所有任务状态</p>
            </div>
            <div className="flex items-center gap-3">
              <Button variant="outline" onClick={refreshData}>
                <RefreshCw className="h-4 w-4 mr-2" />
                刷新
              </Button>
              <Button onClick={handleCreateTask}>
                <Plus className="h-4 w-4 mr-2" />
                创建任务
              </Button>
            </div>
          </div>

          {/* 连接状态 */}
          <div className="mb-6">
            <div className="flex items-center gap-2">
              <div className={`w-2 h-2 rounded-full ${isConnected ? 'bg-green-500' : 'bg-red-500'}`} />
              <span className="text-sm text-muted-foreground">
                {isConnected ? '已连接' : '连接断开'} · 最后更新: {lastUpdate}
              </span>
            </div>
          </div>

          {/* 主要内容 */}
          <Tabs value={activeTab} onValueChange={setActiveTab}>
            <TabsList className="grid w-full grid-cols-3">
              <TabsTrigger value="dashboard">监控面板</TabsTrigger>
              <TabsTrigger value="tasks">任务列表</TabsTrigger>
              <TabsTrigger value="history">历史记录</TabsTrigger>
            </TabsList>

            <TabsContent value="dashboard" className="space-y-6">
              <TaskMonitorDashboard />
            </TabsContent>

            <TabsContent value="tasks" className="space-y-6">
              {/* 搜索和过滤 */}
              <Card>
                <CardHeader>
                  <CardTitle>任务列表</CardTitle>
                </CardHeader>
                <CardContent>
                  <div className="flex items-center gap-4 mb-4">
                    <div className="flex-1">
                      <div className="relative">
                        <Search className="absolute left-3 top-3 h-4 w-4 text-muted-foreground" />
                        <Input
                          placeholder="搜索任务名称..."
                          value={searchTerm}
                          onChange={(e) => setSearchTerm(e.target.value)}
                          className="pl-10"
                        />
                      </div>
                    </div>
                    <Select value={statusFilter} onValueChange={setStatusFilter}>
                      <SelectTrigger className="w-40">
                        <SelectValue placeholder="状态" />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="all">全部状态</SelectItem>
                        <SelectItem value="running">运行中</SelectItem>
                        <SelectItem value="pending">等待中</SelectItem>
                        <SelectItem value="completed">已完成</SelectItem>
                        <SelectItem value="failed">失败</SelectItem>
                        <SelectItem value="paused">已暂停</SelectItem>
                      </SelectContent>
                    </Select>
                    <Select value={typeFilter} onValueChange={setTypeFilter}>
                      <SelectTrigger className="w-40">
                        <SelectValue placeholder="类型" />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="all">全部类型</SelectItem>
                        <SelectItem value="DataParsingWizard">数据解析</SelectItem>
                        <SelectItem value="ModelGeneration">模型生成</SelectItem>
                        <SelectItem value="SpatialTreeGeneration">空间树生成</SelectItem>
                        <SelectItem value="FullSync">全量同步</SelectItem>
                        <SelectItem value="IncrementalSync">增量同步</SelectItem>
                      </SelectContent>
                    </Select>
                  </div>

                  {/* 任务列表 */}
                  <div className="space-y-3">
                    {filteredTasks.map((task) => (
                      <Card key={task.id} className="hover:shadow-md transition-shadow">
                        <CardContent className="p-4">
                          <div className="flex items-center justify-between">
                            <div className="flex-1">
                              <div className="flex items-center gap-3 mb-2">
                                <h3 className="font-semibold">{task.name}</h3>
                                <Badge className={getStatusColor(task.status)}>
                                  {getStatusText(task.status)}
                                </Badge>
                                <Badge variant="outline">
                                  {getTypeText(task.type)}
                                </Badge>
                              </div>
                              <div className="flex items-center gap-4 text-sm text-muted-foreground">
                                <span>ID: {task.id}</span>
                                <span>优先级: {task.priority}</span>
                                <span>进度: {task.progress}%</span>
                                <span>开始时间: {task.startTime}</span>
                                {task.endTime && <span>结束时间: {task.endTime}</span>}
                              </div>
                              {task.error && (
                                <div className="mt-2 text-sm text-red-600">
                                  错误: {task.error}
                                </div>
                              )}
                            </div>
                            <div className="flex items-center gap-2">
                              {task.status === 'running' && (
                                <>
                                  <Button
                                    size="sm"
                                    variant="outline"
                                    onClick={() => handleTaskAction(task.id, 'pause')}
                                  >
                                    <Pause className="h-4 w-4" />
                                  </Button>
                                  <Button
                                    size="sm"
                                    variant="outline"
                                    onClick={() => handleTaskAction(task.id, 'stop')}
                                  >
                                    <Square className="h-4 w-4" />
                                  </Button>
                                </>
                              )}
                              {task.status === 'pending' && (
                                <Button
                                  size="sm"
                                  variant="outline"
                                  onClick={() => handleTaskAction(task.id, 'start')}
                                >
                                  <Play className="h-4 w-4" />
                                </Button>
                              )}
                              {task.status === 'paused' && (
                                <Button
                                  size="sm"
                                  variant="outline"
                                  onClick={() => handleTaskAction(task.id, 'start')}
                                >
                                  <Play className="h-4 w-4" />
                                </Button>
                              )}
                              <Button size="sm" variant="outline">
                                <Eye className="h-4 w-4" />
                              </Button>
                              <Button size="sm" variant="outline">
                                <MoreHorizontal className="h-4 w-4" />
                              </Button>
                            </div>
                          </div>
                        </CardContent>
                      </Card>
                    ))}
                    {filteredTasks.length === 0 && (
                      <div className="text-center py-8 text-muted-foreground">
                        没有找到匹配的任务
                      </div>
                    )}
                  </div>
                </CardContent>
              </Card>
            </TabsContent>

            <TabsContent value="history" className="space-y-6">
              <Card>
                <CardHeader>
                  <CardTitle>任务历史</CardTitle>
                </CardHeader>
                <CardContent>
                  <div className="text-center py-8 text-muted-foreground">
                    任务历史功能开发中...
                  </div>
                </CardContent>
              </Card>
            </TabsContent>
          </Tabs>
        </div>
      </div>
    </div>
  )
}







