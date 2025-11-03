"use client"

import { useState, useEffect, useCallback } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { Badge } from "@/components/ui/badge"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { 
  Search, 
  CheckSquare, 
  Square, 
  Filter,
  RefreshCw,
  AlertTriangle,
  CheckCircle,
  Clock,
  Play,
  Pause
} from "lucide-react"
import { TaskSelectionCard } from "./TaskSelectionCard"
import { useBatchSelection } from "@/hooks/use-batch-selection"
import { useTaskList } from "@/hooks/use-task-list"
import type { Task, TaskStatus, TaskType } from "@/types/task-monitor"

interface BatchTaskSelectorProps {
  onSelectionChange: (selectedIds: string[]) => void
  onTaskAction?: (taskId: string, action: string) => void
}

export function BatchTaskSelector({ onSelectionChange, onTaskAction }: BatchTaskSelectorProps) {
  const [searchQuery, setSearchQuery] = useState("")
  const [statusFilter, setStatusFilter] = useState<TaskStatus | "all">("all")
  const [typeFilter, setTypeFilter] = useState<TaskType | "all">("all")
  const [showFilters, setShowFilters] = useState(false)

  const {
    tasks,
    loading,
    error,
    loadTasks,
    refreshTasks
  } = useTaskList()

  const {
    selectedTasks,
    selectAll,
    selectNone,
    toggleTask,
    isSelected,
    isAllSelected,
    isIndeterminate
  } = useBatchSelection(tasks)

  // 过滤任务
  const filteredTasks = tasks.filter(task => {
    if (searchQuery && !task.name.toLowerCase().includes(searchQuery.toLowerCase())) {
      return false
    }
    if (statusFilter !== "all" && task.status !== statusFilter) {
      return false
    }
    if (typeFilter !== "all" && task.type !== typeFilter) {
      return false
    }
    return true
  })

  // 选择变化时通知父组件
  useEffect(() => {
    onSelectionChange(selectedTasks)
  }, [selectedTasks, onSelectionChange])

  // 加载任务列表
  useEffect(() => {
    loadTasks()
  }, [loadTasks])

  // 处理全选/取消全选
  const handleSelectAll = useCallback(() => {
    if (isAllSelected) {
      selectNone()
    } else {
      selectAll()
    }
  }, [isAllSelected, selectAll, selectNone])

  // 处理搜索
  const handleSearch = useCallback((query: string) => {
    setSearchQuery(query)
  }, [])

  // 处理过滤
  const handleFilterChange = useCallback((filter: string, value: string) => {
    if (filter === 'status') {
      setStatusFilter(value as TaskStatus | "all")
    } else if (filter === 'type') {
      setTypeFilter(value as TaskType | "all")
    }
  }, [])

  // 获取状态统计
  const getStatusCounts = () => {
    const counts = {
      all: tasks.length,
      pending: tasks.filter(t => t.status === 'pending').length,
      running: tasks.filter(t => t.status === 'running').length,
      completed: tasks.filter(t => t.status === 'completed').length,
      failed: tasks.filter(t => t.status === 'failed').length,
      paused: tasks.filter(t => t.status === 'paused').length
    }
    return counts
  }

  const statusCounts = getStatusCounts()

  return (
    <div className="space-y-4">
      {/* 头部控制栏 */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex items-center justify-between">
            <CardTitle className="text-lg">批量任务选择</CardTitle>
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={refreshTasks}
                disabled={loading}
              >
                <RefreshCw className={`h-4 w-4 mr-2 ${loading ? 'animate-spin' : ''}`} />
                刷新
              </Button>
            </div>
          </div>
        </CardHeader>
        <CardContent className="space-y-4">
          {/* 搜索和过滤 */}
          <div className="flex items-center gap-2">
            <div className="flex-1">
              <Input
                placeholder="搜索任务名称..."
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
          </div>

          {/* 过滤条件 */}
          {showFilters && (
            <div className="flex items-center gap-4 p-4 bg-muted/50 rounded-lg">
              <div className="flex items-center gap-2">
                <label className="text-sm font-medium">状态:</label>
                <Select value={statusFilter} onValueChange={(value) => handleFilterChange('status', value)}>
                  <SelectTrigger className="w-32">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">全部 ({statusCounts.all})</SelectItem>
                    <SelectItem value="pending">等待 ({statusCounts.pending})</SelectItem>
                    <SelectItem value="running">运行 ({statusCounts.running})</SelectItem>
                    <SelectItem value="completed">完成 ({statusCounts.completed})</SelectItem>
                    <SelectItem value="failed">失败 ({statusCounts.failed})</SelectItem>
                    <SelectItem value="paused">暂停 ({statusCounts.paused})</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              
              <div className="flex items-center gap-2">
                <label className="text-sm font-medium">类型:</label>
                <Select value={typeFilter} onValueChange={(value) => handleFilterChange('type', value)}>
                  <SelectTrigger className="w-40">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">全部类型</SelectItem>
                    <SelectItem value="ModelGeneration">模型生成</SelectItem>
                    <SelectItem value="SpatialTreeGeneration">空间树生成</SelectItem>
                    <SelectItem value="FullSync">全量同步</SelectItem>
                    <SelectItem value="IncrementalSync">增量同步</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>
          )}

          {/* 选择控制 */}
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Checkbox
                checked={isAllSelected}
                ref={(el) => {
                  if (el) el.indeterminate = isIndeterminate
                }}
                onCheckedChange={handleSelectAll}
              />
              <span className="text-sm font-medium">
                {isAllSelected ? '取消全选' : '全选'}
              </span>
              {selectedTasks.length > 0 && (
                <Badge variant="secondary">
                  已选择 {selectedTasks.length} 个任务
                </Badge>
              )}
            </div>
            
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={selectAll}
                disabled={filteredTasks.length === 0}
              >
                全选当前页
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={selectNone}
                disabled={selectedTasks.length === 0}
              >
                取消选择
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>

      {/* 任务列表 */}
      <div className="space-y-2">
        {loading ? (
          <Card>
            <CardContent className="p-8 text-center">
              <RefreshCw className="h-4 w-4 mx-auto mb-2 animate-spin" />
              <span>加载任务中...</span>
            </CardContent>
          </Card>
        ) : error ? (
          <Card>
            <CardContent className="p-8 text-center text-red-600">
              <AlertTriangle className="h-4 w-4 mx-auto mb-2" />
              <span>{error}</span>
            </CardContent>
          </Card>
        ) : filteredTasks.length === 0 ? (
          <Card>
            <CardContent className="p-8 text-center text-muted-foreground">
              <span>暂无任务数据</span>
            </CardContent>
          </Card>
        ) : (
          filteredTasks.map((task) => (
            <TaskSelectionCard
              key={task.id}
              task={task}
              isSelected={isSelected(task.id)}
              onToggle={() => toggleTask(task.id)}
              onTaskAction={onTaskAction}
            />
          ))
        )}
      </div>

      {/* 底部统计 */}
      <div className="flex items-center justify-between text-sm text-muted-foreground">
        <div>
          显示 {filteredTasks.length} 个任务，共 {tasks.length} 个
        </div>
        <div className="flex items-center gap-4">
          <span className="flex items-center gap-1">
            <Clock className="h-3 w-3" />
            等待: {statusCounts.pending}
          </span>
          <span className="flex items-center gap-1">
            <Play className="h-3 w-3" />
            运行: {statusCounts.running}
          </span>
          <span className="flex items-center gap-1">
            <CheckCircle className="h-3 w-3" />
            完成: {statusCounts.completed}
          </span>
          <span className="flex items-center gap-1">
            <AlertTriangle className="h-3 w-3" />
            失败: {statusCounts.failed}
          </span>
        </div>
      </div>
    </div>
  )
}
