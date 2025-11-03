"use client"

import { useState } from "react"
import { useRouter } from "next/navigation"
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { 
  Plus, 
  Database, 
  Zap, 
  Settings, 
  Clock,
  CheckCircle2,
  ArrowRight,
  Play
} from "lucide-react"
import { TaskCreationWizard } from "@/components/task-creation/TaskCreationWizard"
import { Sidebar } from "@/components/sidebar"

const TASK_TEMPLATES = [
  {
    id: 'data-parsing',
    name: '数据解析任务',
    description: '解析PDMS数据库文件，提取几何和属性信息',
    icon: <Database className="h-6 w-6" />,
    color: 'bg-blue-100 text-blue-800',
    estimatedTime: '10-30分钟',
    taskType: 'DataParsingWizard' as const
  },
  {
    id: 'model-generation',
    name: '模型生成任务',
    description: '基于解析数据生成3D模型和网格文件',
    icon: <Zap className="h-6 w-6" />,
    color: 'bg-green-100 text-green-800',
    estimatedTime: '30-60分钟',
    taskType: 'ModelGeneration' as const
  },
  {
    id: 'spatial-tree',
    name: '空间树生成任务',
    description: '构建空间索引树，优化查询性能',
    icon: <Settings className="h-6 w-6" />,
    color: 'bg-purple-100 text-purple-800',
    estimatedTime: '5-15分钟',
    taskType: 'SpatialTreeGeneration' as const
  },
  {
    id: 'full-sync',
    name: '全量同步任务',
    description: '完整同步所有数据到目标数据库',
    icon: <Database className="h-6 w-6" />,
    color: 'bg-orange-100 text-orange-800',
    estimatedTime: '60-120分钟',
    taskType: 'FullSync' as const
  },
  {
    id: 'incremental-sync',
    name: '增量同步任务',
    description: '仅同步变更的数据到目标数据库',
    icon: <Clock className="h-6 w-6" />,
    color: 'bg-yellow-100 text-yellow-800',
    estimatedTime: '5-20分钟',
    taskType: 'IncrementalSync' as const
  }
]

export default function TaskCreationPage() {
  const router = useRouter()
  const [showWizard, setShowWizard] = useState(false)
  const [selectedTemplate, setSelectedTemplate] = useState<string | null>(null)
  const [taskCreated, setTaskCreated] = useState(false)
  const [createdTaskId, setCreatedTaskId] = useState<string | null>(null)

  const handleTemplateSelect = (templateId: string) => {
    setSelectedTemplate(templateId)
    setShowWizard(true)
  }

  const handleTaskCreated = (taskId: string) => {
    setCreatedTaskId(taskId)
    setTaskCreated(true)
    setShowWizard(false)
    
    // 3秒后跳转到任务监控页面
    setTimeout(() => {
      router.push('/task-monitor')
    }, 3000)
  }

  const handleCancel = () => {
    setShowWizard(false)
    setSelectedTemplate(null)
  }

  const handleCloseSuccessDialog = () => {
    setTaskCreated(false)
    setCreatedTaskId(null)
    router.push('/task-monitor')
  }

  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      
      <div className="ml-64 p-8">
        <div className="max-w-6xl mx-auto">
          {/* 页面头部 */}
          <div className="mb-8">
            <div className="flex items-center gap-3 mb-4">
              <div className="w-12 h-12 bg-primary/10 rounded-lg flex items-center justify-center">
                <Plus className="h-6 w-6 text-primary" />
              </div>
              <div>
                <h1 className="text-3xl font-bold text-foreground">创建新任务</h1>
                <p className="text-muted-foreground">选择任务类型并配置参数</p>
              </div>
            </div>
          </div>

          {/* 任务模板选择 */}
          <div className="mb-8">
            <h2 className="text-xl font-semibold mb-4">选择任务类型</h2>
            <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
              {TASK_TEMPLATES.map((template) => (
                <Card 
                  key={template.id}
                  className="cursor-pointer hover:shadow-lg transition-shadow"
                  onClick={() => handleTemplateSelect(template.id)}
                >
                  <CardHeader>
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-3">
                        <div className="w-10 h-10 bg-primary/10 rounded-lg flex items-center justify-center">
                          {template.icon}
                        </div>
                        <div>
                          <CardTitle className="text-lg">{template.name}</CardTitle>
                          <CardDescription className="text-sm">
                            {template.description}
                          </CardDescription>
                        </div>
                      </div>
                      <ArrowRight className="h-5 w-5 text-muted-foreground" />
                    </div>
                  </CardHeader>
                  <CardContent>
                    <div className="flex items-center justify-between">
                      <Badge className={template.color}>
                        预计 {template.estimatedTime}
                      </Badge>
                      <Button size="sm" variant="outline">
                        <Play className="h-4 w-4 mr-1" />
                        创建
                      </Button>
                    </div>
                  </CardContent>
                </Card>
              ))}
            </div>
          </div>

          {/* 快速操作 */}
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <Card>
              <CardHeader>
                <CardTitle className="flex items-center gap-2">
                  <Database className="h-5 w-5" />
                  批量任务创建
                </CardTitle>
                <CardDescription>
                  为多个项目同时创建相同类型的任务
                </CardDescription>
              </CardHeader>
              <CardContent>
                <Button 
                  variant="outline" 
                  className="w-full"
                  onClick={() => router.push('/batch-tasks')}
                >
                  前往批量任务
                </Button>
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle className="flex items-center gap-2">
                  <Settings className="h-5 w-5" />
                  任务模板管理
                </CardTitle>
                <CardDescription>
                  创建和管理自定义任务模板
                </CardDescription>
              </CardHeader>
              <CardContent>
                <Button 
                  variant="outline" 
                  className="w-full"
                  onClick={() => router.push('/task-templates')}
                >
                  管理模板
                </Button>
              </CardContent>
            </Card>
          </div>
        </div>
      </div>

      {/* 任务创建向导对话框 */}
      <Dialog open={showWizard} onOpenChange={setShowWizard}>
        <DialogContent className="max-w-5xl max-h-[90vh] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>
              {selectedTemplate && TASK_TEMPLATES.find(t => t.id === selectedTemplate)?.name}
            </DialogTitle>
          </DialogHeader>
          <TaskCreationWizard
            onTaskCreated={handleTaskCreated}
            onCancel={handleCancel}
          />
        </DialogContent>
      </Dialog>

      {/* 任务创建成功对话框 */}
      <Dialog open={taskCreated} onOpenChange={setTaskCreated}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <CheckCircle2 className="h-5 w-5 text-green-600" />
              任务创建成功
            </DialogTitle>
          </DialogHeader>
          <div className="space-y-4">
            <p className="text-sm text-muted-foreground">
              任务已成功创建，ID: <code className="bg-muted px-2 py-1 rounded">{createdTaskId}</code>
            </p>
            <p className="text-sm text-muted-foreground">
              正在跳转到任务监控页面...
            </p>
            <div className="flex justify-end gap-2">
              <Button variant="outline" onClick={handleCloseSuccessDialog}>
                立即跳转
              </Button>
            </div>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  )
}







