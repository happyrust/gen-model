"use client"

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Progress } from "@/components/ui/progress"
import { Button } from "@/components/ui/button"
import { FileText, Clock, CheckCircle2, XCircle, Loader2, X } from "lucide-react"
import { cancelTask } from "@/lib/api/remote-sync"
import { toast } from "sonner"
import type { SyncTask } from "@/types/remote-sync"

interface TaskListProps {
  tasks: SyncTask[]
  maxHeight?: string
}

export function TaskList({ tasks, maxHeight = "600px" }: TaskListProps) {

  const getStatusIcon = (status: string) => {
    switch (status) {
      case 'running':
        return <Loader2 className="h-4 w-4 animate-spin text-blue-500" />
      case 'completed':
        return <CheckCircle2 className="h-4 w-4 text-green-500" />
      case 'failed':
        return <XCircle className="h-4 w-4 text-red-500" />
      case 'cancelled':
        return <X className="h-4 w-4 text-gray-500" />
      default:
        return <Clock className="h-4 w-4 text-gray-400" />
    }
  }

  const getStatusText = (status: string) => {
    switch (status) {
      case 'pending':
        return '等待中'
      case 'running':
        return '运行中'
      case 'completed':
        return '已完成'
      case 'failed':
        return '失败'
      case 'cancelled':
        return '已取消'
      default:
        return status
    }
  }

  const getStatusVariant = (status: string): "default" | "secondary" | "destructive" | "outline" => {
    switch (status) {
      case 'running':
        return 'default'
      case 'completed':
        return 'secondary'
      case 'failed':
        return 'destructive'
      default:
        return 'outline'
    }
  }

  const formatFileSize = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  }

  const handleCancel = async (taskId: string) => {
    try {
      await cancelTask(taskId)
      toast.success('任务已取消')
    } catch (error) {
      toast.error('取消任务失败')
    }
  }

  if (tasks.length === 0) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>同步任务</CardTitle>
          <CardDescription>实时显示同步任务状态</CardDescription>
        </CardHeader>
        <CardContent>
          <div className="text-center py-8 text-muted-foreground">
            <FileText className="h-12 w-12 mx-auto mb-2 opacity-50" />
            <p>暂无同步任务</p>
          </div>
        </CardContent>
      </Card>
    )
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>同步任务</CardTitle>
        <CardDescription>
          共 {tasks.length} 个任务
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div className="space-y-3" style={{ maxHeight, overflowY: 'auto' }}>
          {tasks.map((task) => (
            <div
              key={task.id}
              className="p-4 border rounded-lg hover:bg-accent transition-colors"
            >
              <div className="flex items-start justify-between mb-2">
                <div className="flex items-center gap-2 flex-1 min-w-0">
                  {getStatusIcon(task.status)}
                  <div className="flex-1 min-w-0">
                    <div className="font-medium truncate">{task.fileName}</div>
                    <div className="text-xs text-muted-foreground">
                      {task.sourceEnv} → {task.targetSite}
                    </div>
                  </div>
                </div>
                <div className="flex items-center gap-2 ml-2">
                  <Badge variant={getStatusVariant(task.status)}>
                    {getStatusText(task.status)}
                  </Badge>
                  {task.status === 'running' && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => handleCancel(task.id)}
                    >
                      <X className="h-4 w-4" />
                    </Button>
                  )}
                </div>
              </div>

              {/* 进度条 */}
              {task.status === 'running' && (
                <div className="mb-2">
                  <Progress value={task.progress} className="h-1" />
                  <div className="text-xs text-muted-foreground mt-1">
                    {task.progress}%
                  </div>
                </div>
              )}

              {/* 详细信息 */}
              <div className="flex items-center gap-4 text-xs text-muted-foreground">
                <span>{formatFileSize(task.fileSize)}</span>
                <span>{task.recordCount} 条记录</span>
                <span>优先级: {task.priority}</span>
                {task.retryCount > 0 && (
                  <span className="text-yellow-600">重试: {task.retryCount}</span>
                )}
              </div>

              {/* 错误信息 */}
              {task.errorMessage && (
                <div className="mt-2 p-2 bg-destructive/10 text-destructive text-xs rounded">
                  {task.errorMessage}
                </div>
              )}
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}
