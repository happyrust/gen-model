"use client"

import { useState, useCallback, useMemo } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Label } from "@/components/ui/label"
import { Badge } from "@/components/ui/badge"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import {
  Search,
  Filter,
  RefreshCw,
  Download,
  Play,
  Eye,
  Clock,
  CheckCircle,
  AlertTriangle,
  XCircle,
} from "lucide-react"
import { useTaskHistory } from "@/hooks/use-task-history"
import type { TaskHistory, HistoryFilters, TaskStatistics } from "@/types/task-history"

interface TaskHistoryListProps {
  onTaskSelect?: (taskId: string) => void
  onReplay?: (taskId: string) => void
}

interface TaskHistoryCardProps {
  task: TaskHistory
  onViewDetails?: () => void
  onReplay?: () => void
}

interface TaskHistoryFiltersProps {
  filters: HistoryFilters
  onFilterChange: (filters: Partial<HistoryFilters>) => void
  availableStatuses: HistoryFilters["status"][]
  availableTypes: string[]
}

interface TaskAnalyticsProps {
  statistics: TaskStatistics | null
  charts: Array<{ date: string; total: number; completed: number; failed: number }>
  loading?: boolean
}

const STATUS_BADGES: Record<string, string> = {
  completed: "bg-green-100 text-green-800",
  failed: "bg-red-100 text-red-800",
  cancelled: "bg-gray-100 text-gray-800",
  running: "bg-blue-100 text-blue-800",
  pending: "bg-yellow-100 text-yellow-800",
  unknown: "bg-muted text-muted-foreground",
}

const TASK_TYPE_LABELS: Record<string, string> = {
  ModelGeneration: "模型生成",
  SpatialTreeGeneration: "空间树生成",
  FullSync: "全量同步",
  IncrementalSync: "增量同步",
}

function TaskHistoryCard({ task, onViewDetails, onReplay }: TaskHistoryCardProps) {
  const badgeClass = STATUS_BADGES[task.status] ?? STATUS_BADGES.unknown
  const startTimeLabel = task.startTime
    ? new Date(task.startTime).toLocaleString()
    : task.createdAt
      ? new Date(task.createdAt).toLocaleString()
      : "未知"
  const endTimeLabel = task.endTime
    ? new Date(task.endTime).toLocaleString()
    : "未完成"
  const durationSeconds =
    task.durationMs !== undefined
      ? (task.durationMs / 1000).toFixed(1)
      : "0"
  return (
    <Card className="transition-colors hover:border-primary">
      <CardContent className="flex flex-col gap-3 py-4 md:flex-row md:items-center md:justify-between">
        <div className="space-y-2">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm font-semibold text-foreground">{task.name}</span>
            <Badge variant="secondary">{TASK_TYPE_LABELS[task.type] ?? task.type}</Badge>
            <span className={`rounded px-2 py-0.5 text-xs font-medium ${badgeClass}`}>
              {task.status === "completed"
                ? "已完成"
                : task.status === "failed"
                  ? "失败"
                  : task.status === "cancelled"
                    ? "已取消"
                    : task.status === "running"
                      ? "运行中"
                      : "等待中"}
            </span>
          </div>
          <div className="flex flex-wrap items-center gap-4 text-xs text-muted-foreground">
            <span>ID: {task.id}</span>
            <span>任务ID: {task.taskId}</span>
            <span>开始时间: {startTimeLabel}</span>
            <span>结束时间: {endTimeLabel}</span>
            <span>耗时: {durationSeconds}s</span>
          </div>
          {task.result?.message && (
            <p className="text-xs text-muted-foreground">备注: {task.result.message}</p>
          )}
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="sm" onClick={onViewDetails} className="gap-1">
            <Eye className="h-4 w-4" />
            详情
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={onReplay}
            disabled={task.status === "running"}
            className="gap-1"
          >
            <Play className="h-4 w-4" />
            重新执行
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}

function TaskHistoryFilters({
  filters,
  onFilterChange,
  availableStatuses,
  availableTypes,
}: TaskHistoryFiltersProps) {
  return (
    <div className="mt-4 grid gap-4 rounded-lg border border-border/60 bg-muted/30 p-4 md:grid-cols-3">
      <div className="space-y-1">
        <Label className="text-xs text-muted-foreground">状态</Label>
        <Select
          value={filters.status}
          onValueChange={(value) => onFilterChange({ status: value as HistoryFilters["status"] })}
        >
          <SelectTrigger>
            <SelectValue placeholder="选择状态" />
          </SelectTrigger>
          <SelectContent>
            {["all", ...availableStatuses].map((status) => (
              <SelectItem key={status} value={status}>
                {status === "all"
                  ? "全部"
                  : status === "completed"
                    ? "已完成"
                    : status === "failed"
                      ? "失败"
                      : status === "cancelled"
                        ? "已取消"
                        : status === "running"
                          ? "运行中"
                          : status === "pending"
                            ? "等待中"
                            : "未知"}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="space-y-1">
        <Label className="text-xs text-muted-foreground">类型</Label>
        <Select
          value={filters.type}
          onValueChange={(value) => onFilterChange({ type: value as HistoryFilters["type"] })}
        >
          <SelectTrigger>
            <SelectValue placeholder="选择类型" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">全部类型</SelectItem>
            {availableTypes.map((type) => (
              <SelectItem key={type} value={type}>
                {TASK_TYPE_LABELS[type] ?? type}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="space-y-1">
        <Label className="text-xs text-muted-foreground">排序</Label>
        <Select
          value={filters.sortBy}
          onValueChange={(value) => onFilterChange({ sortBy: value as HistoryFilters["sortBy"] })}
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="startTime">开始时间</SelectItem>
            <SelectItem value="endTime">结束时间</SelectItem>
            <SelectItem value="duration">耗时</SelectItem>
            <SelectItem value="status">状态</SelectItem>
          </SelectContent>
        </Select>
      </div>
    </div>
  )
}

function TaskAnalytics({ statistics, charts, loading }: TaskAnalyticsProps) {
  const isLoading = loading ?? false
  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>任务统计概览</CardTitle>
        </CardHeader>
        <CardContent className="grid grid-cols-1 gap-4 md:grid-cols-3">
          <div>
            <p className="text-xs text-muted-foreground">任务总数</p>
            <p className="text-2xl font-bold">{statistics?.total ?? 0}</p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">成功率</p>
            <p className="text-2xl font-bold text-green-600">
              {statistics ? `${(statistics.successRate * 100).toFixed(1)}%` : "0%"}
            </p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">平均耗时 (秒)</p>
            <p className="text-2xl font-bold">{statistics ? (statistics.avgDuration / 1000).toFixed(1) : "0.0"}</p>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>趋势图</CardTitle>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <div className="py-8 text-center text-muted-foreground">加载分析数据...</div>
          ) : charts.length === 0 ? (
            <div className="py-8 text-center text-muted-foreground">暂无趋势数据</div>
          ) : (
            <div className="space-y-2 text-sm text-muted-foreground">
              {charts.map((entry) => (
                <div key={entry.date} className="flex items-center justify-between rounded border border-border/60 p-2">
                  <span>{entry.date}</span>
                  <span>
                    总计 {entry.total} · 完成 {entry.completed} · 失败 {entry.failed}
                  </span>
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}

export function TaskHistoryList({ onTaskSelect, onReplay }: TaskHistoryListProps) {
  const [activeTab, setActiveTab] = useState("list")
  const [searchQuery, setSearchQuery] = useState("")
  const [showFilters, setShowFilters] = useState(false)

  const {
    tasks,
    allTasks,
    loading,
    error,
    filters,
    pagination,
    refreshHistory,
    setFilters,
    setPagination,
    replayTask,
    getTaskStatistics,
    getStatisticsByType,
    getStatisticsByDate,
  } = useTaskHistory()

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
    void replayTask(taskId)
  }, [onReplay, replayTask])

  const summary = useMemo<TaskStatistics>(
    () => getTaskStatistics(),
    [getTaskStatistics]
  )

  const analyticsStatistics = useMemo<TaskStatistics | null>(
    () => (summary.total > 0 ? summary : null),
    [summary]
  )

  const analyticsChartData = useMemo(
    () => getStatisticsByDate(),
    [getStatisticsByDate]
  )

  const availableTypes = useMemo<string[]>(
    () =>
      Array.from(
        new Set(
          allTasks
            .map((task) => task.type)
            .filter((type) => type && type.length > 0)
        )
      ),
    [allTasks]
  )

  const availableStatuses = useMemo<HistoryFilters["status"][]>(
    () => {
      const base = new Set<HistoryFilters["status"]>([
        "completed",
        "failed",
        "cancelled",
        "running",
        "pending",
      ])
      allTasks.forEach((task) => {
        if (task.status !== "unknown") {
          base.add(task.status as HistoryFilters["status"])
        }
      })
      return Array.from(base)
    },
    [allTasks]
  )

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
              availableStatuses={availableStatuses}
              availableTypes={availableTypes}
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
                <div className="text-2xl font-bold text-green-600">{summary.completed}</div>
              </CardContent>
            </Card>
            
            <Card>
              <CardContent className="p-4">
                <div className="flex items-center gap-2">
                  <AlertTriangle className="h-4 w-4 text-red-600" />
                  <span className="text-sm font-medium">失败</span>
                </div>
                <div className="text-2xl font-bold text-red-600">{summary.failed}</div>
              </CardContent>
            </Card>
            
            <Card>
              <CardContent className="p-4">
                <div className="flex items-center gap-2">
                  <XCircle className="h-4 w-4 text-gray-600" />
                  <span className="text-sm font-medium">已取消</span>
                </div>
                <div className="text-2xl font-bold text-gray-600">{summary.cancelled}</div>
              </CardContent>
            </Card>
            
            <Card>
              <CardContent className="p-4">
                <div className="flex items-center gap-2">
                  <Clock className="h-4 w-4 text-blue-600" />
                  <span className="text-sm font-medium">运行中</span>
                </div>
                <div className="text-2xl font-bold text-blue-600">{summary.running}</div>
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
            statistics={analyticsStatistics}
            charts={analyticsChartData}
            loading={loading}
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
