'use client'

import { useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { useToast } from '@/hooks/use-toast'
import {
  Play,
  Square,
  Pause,
  RotateCcw,
  Trash2,
  Plus,
  Loader2,
} from 'lucide-react'

interface OpsToolbarProps {
  onRefresh?: () => void
  showAddTask?: boolean
}

export function OpsToolbar({ onRefresh, showAddTask = false }: OpsToolbarProps) {
  const [isLoading, setIsLoading] = useState(false)
  const [confirmAction, setConfirmAction] = useState<{
    action: string
    title: string
    description: string
    onConfirm: () => Promise<void>
  } | null>(null)
  const [showAddTaskDialog, setShowAddTaskDialog] = useState(false)
  const [taskForm, setTaskForm] = useState({
    file_path: '',
    direction: 'push',
    notes: '',
  })
  const { toast } = useToast()

  // 启动同步服务
  const handleStart = async () => {
    setIsLoading(true)
    try {
      const response = await fetch('/api/sync/start', { method: 'POST' })
      if (!response.ok) throw new Error('启动失败')
      
      toast({
        title: '启动成功',
        description: '同步服务已启动',
      })
      onRefresh?.()
    } catch (error) {
      toast({
        title: '启动失败',
        description: error instanceof Error ? error.message : '未知错误',
        variant: 'destructive',
      })
    } finally {
      setIsLoading(false)
      setConfirmAction(null)
    }
  }

  // 停止同步服务
  const handleStop = async () => {
    setIsLoading(true)
    try {
      const response = await fetch('/api/sync/stop', { method: 'POST' })
      if (!response.ok) throw new Error('停止失败')
      
      toast({
        title: '停止成功',
        description: '同步服务已停止',
      })
      onRefresh?.()
    } catch (error) {
      toast({
        title: '停止失败',
        description: error instanceof Error ? error.message : '未知错误',
        variant: 'destructive',
      })
    } finally {
      setIsLoading(false)
      setConfirmAction(null)
    }
  }

  // 暂停同步服务
  const handlePause = async () => {
    setIsLoading(true)
    try {
      const response = await fetch('/api/sync/pause', { method: 'POST' })
      if (!response.ok) throw new Error('暂停失败')
      
      toast({
        title: '暂停成功',
        description: '同步服务已暂停',
      })
      onRefresh?.()
    } catch (error) {
      toast({
        title: '暂停失败',
        description: error instanceof Error ? error.message : '未知错误',
        variant: 'destructive',
      })
    } finally {
      setIsLoading(false)
      setConfirmAction(null)
    }
  }

  // 恢复同步服务
  const handleResume = async () => {
    setIsLoading(true)
    try {
      const response = await fetch('/api/sync/resume', { method: 'POST' })
      if (!response.ok) throw new Error('恢复失败')
      
      toast({
        title: '恢复成功',
        description: '同步服务已恢复',
      })
      onRefresh?.()
    } catch (error) {
      toast({
        title: '恢复失败',
        description: error instanceof Error ? error.message : '未知错误',
        variant: 'destructive',
      })
    } finally {
      setIsLoading(false)
      setConfirmAction(null)
    }
  }

  // 清空队列
  const handleClearQueue = async () => {
    setIsLoading(true)
    try {
      const response = await fetch('/api/sync/queue/clear', { method: 'POST' })
      if (!response.ok) throw new Error('清空队列失败')
      
      toast({
        title: '清空成功',
        description: '同步队列已清空',
      })
      onRefresh?.()
    } catch (error) {
      toast({
        title: '清空失败',
        description: error instanceof Error ? error.message : '未知错误',
        variant: 'destructive',
      })
    } finally {
      setIsLoading(false)
      setConfirmAction(null)
    }
  }

  // 添加任务
  const handleAddTask = async () => {
    if (!taskForm.file_path.trim()) {
      toast({
        title: '添加失败',
        description: '请输入文件路径',
        variant: 'destructive',
      })
      return
    }

    setIsLoading(true)
    try {
      const response = await fetch('/api/sync/task', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(taskForm),
      })
      
      if (!response.ok) throw new Error('添加任务失败')
      
      toast({
        title: '添加成功',
        description: '任务已添加到队列',
      })
      
      setShowAddTaskDialog(false)
      setTaskForm({ file_path: '', direction: 'push', notes: '' })
      onRefresh?.()
    } catch (error) {
      toast({
        title: '添加失败',
        description: error instanceof Error ? error.message : '未知错误',
        variant: 'destructive',
      })
    } finally {
      setIsLoading(false)
    }
  }

  return (
    <>
      <div className="flex items-center gap-2 flex-wrap">
        <Button
          onClick={() => setConfirmAction({
            action: 'start',
            title: '启动同步服务',
            description: '确定要启动同步服务吗？',
            onConfirm: handleStart,
          })}
          variant="default"
          size="sm"
          disabled={isLoading}
        >
          {isLoading ? <Loader2 className="w-4 h-4 mr-2 animate-spin" /> : <Play className="w-4 h-4 mr-2" />}
          启动
        </Button>

        <Button
          onClick={() => setConfirmAction({
            action: 'stop',
            title: '停止同步服务',
            description: '确定要停止同步服务吗？正在进行的任务将被中断。',
            onConfirm: handleStop,
          })}
          variant="destructive"
          size="sm"
          disabled={isLoading}
        >
          <Square className="w-4 h-4 mr-2" />
          停止
        </Button>

        <Button
          onClick={() => setConfirmAction({
            action: 'pause',
            title: '暂停同步服务',
            description: '确定要暂停同步服务吗？',
            onConfirm: handlePause,
          })}
          variant="outline"
          size="sm"
          disabled={isLoading}
        >
          <Pause className="w-4 h-4 mr-2" />
          暂停
        </Button>

        <Button
          onClick={() => setConfirmAction({
            action: 'resume',
            title: '恢复同步服务',
            description: '确定要恢复同步服务吗？',
            onConfirm: handleResume,
          })}
          variant="outline"
          size="sm"
          disabled={isLoading}
        >
          <RotateCcw className="w-4 h-4 mr-2" />
          恢复
        </Button>

        <Button
          onClick={() => setConfirmAction({
            action: 'clear',
            title: '清空队列',
            description: '确定要清空同步队列吗？所有待处理的任务将被删除。',
            onConfirm: handleClearQueue,
          })}
          variant="outline"
          size="sm"
          disabled={isLoading}
        >
          <Trash2 className="w-4 h-4 mr-2" />
          清空队列
        </Button>

        {showAddTask && (
          <Button
            onClick={() => setShowAddTaskDialog(true)}
            variant="outline"
            size="sm"
            disabled={isLoading}
          >
            <Plus className="w-4 h-4 mr-2" />
            添加任务
          </Button>
        )}
      </div>

      {/* 确认对话框 */}
      <AlertDialog open={!!confirmAction} onOpenChange={() => setConfirmAction(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{confirmAction?.title}</AlertDialogTitle>
            <AlertDialogDescription>
              {confirmAction?.description}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={isLoading}>取消</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => confirmAction?.onConfirm()}
              disabled={isLoading}
            >
              {isLoading ? '处理中...' : '确认'}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* 添加任务对话框 */}
      <Dialog open={showAddTaskDialog} onOpenChange={setShowAddTaskDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>添加同步任务</DialogTitle>
            <DialogDescription>
              手动添加一个文件同步任务到队列
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4 py-4">
            <div>
              <Label htmlFor="file_path">文件路径 *</Label>
              <Input
                id="file_path"
                placeholder="/path/to/file.cba"
                value={taskForm.file_path}
                onChange={(e) => setTaskForm({ ...taskForm, file_path: e.target.value })}
              />
            </div>
            <div>
              <Label htmlFor="direction">同步方向</Label>
              <select
                id="direction"
                className="w-full border rounded-md p-2"
                value={taskForm.direction}
                onChange={(e) => setTaskForm({ ...taskForm, direction: e.target.value })}
              >
                <option value="push">推送 (Push)</option>
                <option value="pull">拉取 (Pull)</option>
              </select>
            </div>
            <div>
              <Label htmlFor="notes">备注</Label>
              <Textarea
                id="notes"
                placeholder="可选的任务备注..."
                value={taskForm.notes}
                onChange={(e) => setTaskForm({ ...taskForm, notes: e.target.value })}
              />
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setShowAddTaskDialog(false)}>
              取消
            </Button>
            <Button onClick={handleAddTask} disabled={isLoading}>
              {isLoading ? '添加中...' : '添加任务'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
