"use client"

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import type { Task, TaskQueue } from "@/types/task-monitor"

interface TaskQueueMonitorProps {
  queue: Pick<TaskQueue, 'pending' | 'running'>
  onQueueAction?: (action: string) => void
}

export function TaskQueueMonitor({ queue }: TaskQueueMonitorProps) {
  const renderList = (title: string, items: Task[]) => (
    <Card>
      <CardHeader>
        <CardTitle>{title}（{items.length}）</CardTitle>
      </CardHeader>
      <CardContent className="space-y-2">
        {items.map(item => (
          <div key={item.id} className="flex items-center justify-between text-sm">
            <div className="truncate">
              <span className="font-medium mr-2">{item.name}</span>
              <span className="text-muted-foreground">{item.type}</span>
            </div>
            <span className="text-muted-foreground">{item.progress}%</span>
          </div>
        ))}
        {items.length === 0 && (
          <div className="text-sm text-muted-foreground">暂无数据</div>
        )}
      </CardContent>
    </Card>
  )

  return (
    <div className="grid gap-4 md:grid-cols-2">
      {renderList('等待队列', queue.pending)}
      {renderList('运行中', queue.running)}
    </div>
  )
}


