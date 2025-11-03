"use client"

import { useState, useEffect, useCallback } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Badge } from "@/components/ui/badge"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { 
  Search, 
  Filter, 
  RefreshCw, 
  Download, 
  Play, 
  Eye,
  Calendar,
  Clock,
  CheckCircle,
  AlertTriangle,
  XCircle,
  Pause
} from "lucide-react"
import { TaskHistoryCard } from "./TaskHistoryCard"
import { TaskHistoryFilters } from "./TaskHistoryFilters"
import { TaskAnalytics } from "./TaskAnalytics"
import { useTaskHistory } from "@/hooks/use-task-history"
import { useTaskAnalytics } from "@/hooks/use-task-analytics"
import type { TaskHistory, HistoryFilters, TaskStatistics } from "@/types/task-history"

interface TaskHistoryListProps {
  onTaskSelect?: (taskId: string) => void
  onReplay?: (taskId: string) => void
}

export function TaskHistoryList({ onTaskSelect, onReplay }: TaskHistoryListProps) {
  const [activeTab, setActiveTab] = useState("list")
  const [searchQuery, setSearchQuery] = useState("")
  const [showFilters, setShowFilters] = useState(false)
  const [dateRange, setDateRange] = useState<[Date, Date] | null>(null)

  const {
    tasks,
    loading,
    error,
    filters,
    pagination,
    loadHistory,
    setFilters,
    setPagination,
    refreshHistory
  } = useTaskHistory()

  const {
    statistics,
    charts,
    loading: analyticsLoading,
    loadAnalytics
  } = useTaskAnalytics()

  // 加载历史数据
  useEffect(() => {
    loadHistory(filters)
  }, [loadHistory, filters])

  // 加载分析数据
  useEffect(() => {
    if (activeTab === "analytics") {
      loadAnalytics(dateRange)
    }
  }, [activeTab, dateRange, loadAnalytics])

  // 处理搜索
  const handleSearch = useCallback((query: string) => {
    setSearchQuery(query)
    setFilters({ search: query })
  }, [setFilters])

  // 处理过滤条件变化
  const handleFilterChange = useCallback((newFilters: Partial<HistoryFilters>) => {
    setFilters(newFilters)
  }, [setFilters])

  // 处理分页变化
  const handlePageChange = useCallback((page: number) => {
    setPagination({ currentPage: page })
  }, [setPagination])

  // 处理任务选择
  const handleTaskSelect = useCallback((taskId: string) => {
    onTaskSelect?.(taskId)
  }, [onTaskSelect])

  // 处理任务重新执行
  const handleTaskReplay = useCallback((taskId: string) => {
    onReplay?.(taskId)
  }, [onReplay])

  // 处理日期范围变化
  const handleDateRangeChange = useCallback((range: [Date, Date]) => {
    setDateRange(range)
    setFilters({ dateRange: range })
  }, [setFilters])

  // 获取状态统计
  const getStatusCounts = () => {
    const counts = {
      completed: tasks.filter(t => t.status === 'completed').length,
      failed: tasks.filter(t => t.status === 'failed').length,
      cancelled: tasks.filter(t => t.status === 'cancelled').length,
      running: tasks.filter(t => t.status === 'running').length
    }
    return counts
  }

  const statusCounts = getStatusCounts()

  return (
    <div className="space-y-6">
      {/* 头部控制栏 */}
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-2xl font-bold">任务历史</h2>
          <p className="text-muted-foreground">查看和管理历史任务记录</p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={refreshHistory}
            disabled={loading}
          >
            <RefreshCw className={`h-4 w-4 mr-2 ${loading ? 'animate-spin' : ''}`} />
            刷新
          </Button>
        </div>
      </div>

      {/* 搜索和过滤 */}
      <Card>
        <CardContent className="p-4">
          <div className="flex items-center gap-4">
            <div className="flex-1">
              <Input
                placeholder="搜索任务名称或ID..."
                value={searchQuery}
                onChange={(e) => handleSearch(e.target.value)}
                className="max-w-sm"
              />
            </div>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setShowFilters(!showFilters)}
            >
              <Filter className="h-4 w-4 mr-2" />
              过滤
            </Button>
            <Button
              variant="outline"
              size="sm"
            >
              <Download className="h-4 w-4 mr-2" />
              导出
            </Button>
          </div>

          {/* 过滤条件 */}
          {showFilters && (
            <TaskHistoryFilters
              filters={filters}
              onFilterChange={handleFilterChange}
              availableStatuses={['completed', 'failed', 'cancelled']}
              availableTypes={['ModelGeneration', 'SpatialTreeGeneration', 'FullSync', 'IncrementalSync']}
            />
          )}
        </CardContent>
      </Card>

      {/* 主要内容区域 */}
      <Tabs value={activeTab} onValueChange={setActiveTab}>
        <TabsList className="grid w-full grid-cols-3">
          <TabsTrigger value="list">历史列表</TabsTrigger>
          <TabsTrigger value="analytics">数据分析</TabsTrigger>
          <TabsTrigger value="reports">执行报告</TabsTrigger>
        </TabsList>

        <TabsContent value="list" className="space-y-4">
          {/* 状态统计卡片 */}
          <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
            <Card>
              <CardContent className="p-4">
                <div className="flex items-center gap-2">
                  <CheckCircle className="h-4 w-4 text-green-600" />
                  <span className="text-sm font-medium">已完成</span>
                </div>
                <div className="text-2xl font-bold text-green-600">{statusCounts.completed}</div>
              </CardContent>
            </Card>
            
            <Card>
              <CardContent className="p-4">
                <div className="flex items-center gap-2">
                  <AlertTriangle className="h-4 w-4 text-red-600" />
                  <span className="text-sm font-medium">失败</span>
                </div>
                <div className="text-2xl font-bold text-red-600">{statusCounts.failed}</div>
              </CardContent>
            </Card>
            
            <Card>
              <CardContent className="p-4">
                <div className="flex items-center gap-2">
                  <XCircle className="h-4 w-4 text-gray-600" />
                  <span className="text-sm font-medium">已取消</span>
                </div>
                <div className="text-2xl font-bold text-gray-600">{statusCounts.cancelled}</div>
              </CardContent>
            </Card>
            
            <Card>
              <CardContent className="p-4">
                <div className="flex items-center gap-2">
                  <Clock className="h-4 w-4 text-blue-600" />
                  <span className="text-sm font-medium">运行中</span>
                </div>
                <div className="text-2xl font-bold text-blue-600">{statusCounts.running}</div>
              </CardContent>
            </Card>
          </div>

          {/* 任务列表 */}
          <div className="space-y-2">
            {loading ? (
              <Card>
                <CardContent className="p-8 text-center">
                  <RefreshCw className="h-4 w-4 mx-auto mb-2 animate-spin" />
                  <span>加载历史数据中...</span>
                </CardContent>
              </Card>
            ) : error ? (
              <Card>
                <CardContent className="p-8 text-center text-red-600">
                  <AlertTriangle className="h-4 w-4 mx-auto mb-2" />
                  <span>{error}</span>
                </CardContent>
              </Card>
            ) : tasks.length === 0 ? (
              <Card>
                <CardContent className="p-8 text-center text-muted-foreground">
                  <span>暂无历史任务数据</span>
                </CardContent>
              </Card>
            ) : (
              tasks.map((task) => (
                <TaskHistoryCard
                  key={task.id}
                  task={task}
                  onViewDetails={() => handleTaskSelect(task.id)}
                  onReplay={() => handleTaskReplay(task.id)}
                />
              ))
            )}
          </div>

          {/* 分页 */}
          {pagination.totalPages > 1 && (
            <div className="flex items-center justify-center gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={() => handlePageChange(pagination.currentPage - 1)}
                disabled={pagination.currentPage <= 1}
              >
                上一页
              </Button>
              <span className="text-sm text-muted-foreground">
                第 {pagination.currentPage} 页，共 {pagination.totalPages} 页
              </span>
              <Button
                variant="outline"
                size="sm"
                onClick={() => handlePageChange(pagination.currentPage + 1)}
                disabled={pagination.currentPage >= pagination.totalPages}
              >
                下一页
              </Button>
            </div>
          )}
        </TabsContent>

        <TabsContent value="analytics">
          <TaskAnalytics
            statistics={statistics}
            charts={charts}
            loading={analyticsLoading}
            dateRange={dateRange}
            onDateRangeChange={handleDateRangeChange}
          />
        </TabsContent>

        <TabsContent value="reports">
          <Card>
            <CardHeader>
              <CardTitle>执行报告</CardTitle>
            </CardHeader>
            <CardContent>
              <p className="text-muted-foreground">报告功能开发中...</p>
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>
    </div>
  )
}
