"use client"

import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Progress } from "@/components/ui/progress"
import { Play, Pause, Square } from "lucide-react"
import type { Task } from "@/types/task-monitor"

interface TaskStatusCardProps {
  task: Task
  onTaskAction?: (taskId: string, action: string) => void
}

export function TaskStatusCard({ task, onTaskAction }: TaskStatusCardProps) {
  const getStatusColor = (status: Task["status"]) => {
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

  return (
    <Card>
      <CardContent className="p-4">
        <div className="flex items-center justify-between">
          <div className="flex-1">
            <div className="flex items-center gap-3 mb-2">
              <h3 className="font-semibold">{task.name}</h3>
              <Badge className={getStatusColor(task.status)}>{task.status}</Badge>
              <Badge variant="outline">{task.type}</Badge>
            </div>
            <div className="flex items-center gap-4 text-sm text-muted-foreground">
              <span>ID: {task.id}</span>
              <span>优先级: {task.priority}</span>
              <span>进度: {task.progress}%</span>
              <span>开始时间: {task.startTime}</span>
              {task.endTime && <span>结束时间: {task.endTime}</span>}
            </div>
            <div className="mt-3">
              <Progress value={task.progress} />
            </div>
            {task.error && (
              <div className="mt-2 text-sm text-red-600">错误: {task.error}</div>
            )}
          </div>
          <div className="flex items-center gap-2 ml-4">
            {task.status === 'running' && (
              <>
                <Button size="sm" variant="outline" onClick={() => onTaskAction?.(task.id, 'pause')}>
                  <Pause className="h-4 w-4" />
                </Button>
                <Button size="sm" variant="outline" onClick={() => onTaskAction?.(task.id, 'stop')}>
                  <Square className="h-4 w-4" />
                </Button>
              </>
            )}
            {task.status === 'pending' && (
              <Button size="sm" variant="outline" onClick={() => onTaskAction?.(task.id, 'start')}>
                <Play className="h-4 w-4" />
              </Button>
            )}
            {task.status === 'paused' && (
              <Button size="sm" variant="outline" onClick={() => onTaskAction?.(task.id, 'start')}>
                <Play className="h-4 w-4" />
              </Button>
            )}
          </div>
        </div>
      </CardContent>
    </Card>
  )
}


