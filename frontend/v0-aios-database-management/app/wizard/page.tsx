"use client"

import { useState, useEffect } from "react"
import { useRouter } from "next/navigation"
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Label } from "@/components/ui/label"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Checkbox } from "@/components/ui/checkbox"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { Badge } from "@/components/ui/badge"
import { Alert, AlertDescription } from "@/components/ui/alert"
import { Sidebar } from "@/components/sidebar"
import { 
  Database, 
  Zap, 
  ChevronRight, 
  Loader2, 
  CheckCircle2, 
  AlertCircle,
  Settings,
  Package,
  Clock,
  Play,
  ArrowRight,
  Plus
} from "lucide-react"
import { useTaskCreation } from "@/hooks/use-task-creation"
import type { TaskCreationFormData, TaskType, TaskPriority } from "@/types/task-creation"

interface Site {
  id: string
  name: string
  status: string
  environment: string
  description?: string
}

type ParseMode = "all" | "dbnum" | "refno"

interface ParseTaskConfig {
  site_id: string
  task_types: TaskType[]
  parse_mode: ParseMode
  dbnum?: number
  refno?: string
  priority: TaskPriority
  task_name?: string
}

const TASK_TYPES: { value: TaskType; label: string; description: string; icon: React.ReactNode }[] = [
  {
    value: 'DataParsingWizard',
    label: '数据解析任务',
    description: '解析PDMS数据库文件，提取几何和属性信息',
    icon: <Database className="h-5 w-5" />
  },
  {
    value: 'ModelGeneration',
    label: '模型生成任务',
    description: '基于解析数据生成3D模型和网格文件',
    icon: <Zap className="h-5 w-5" />
  },
  {
    value: 'SpatialTreeGeneration',
    label: '空间树生成任务',
    description: '构建空间索引树，优化查询性能',
    icon: <Settings className="h-5 w-5" />
  },
  {
    value: 'FullSync',
    label: '全量同步任务',
    description: '完整同步所有数据到目标数据库',
    icon: <Database className="h-5 w-5" />
  },
  {
    value: 'IncrementalSync',
    label: '增量同步任务',
    description: '仅同步变更的数据到目标数据库',
    icon: <Clock className="h-5 w-5" />
  }
]

const PRIORITY_OPTIONS: { value: TaskPriority; label: string; color: string }[] = [
  { value: 'Low', label: '低', color: 'bg-gray-100 text-gray-800' },
  { value: 'Normal', label: '普通', color: 'bg-blue-100 text-blue-800' },
  { value: 'High', label: '高', color: 'bg-orange-100 text-orange-800' },
  { value: 'Critical', label: '紧急', color: 'bg-red-100 text-red-800' }
]

export default function EnhancedWizardPage() {
  const router = useRouter()
  const [activeTab, setActiveTab] = useState<"quick" | "advanced">("quick")
  const [currentStep, setCurrentStep] = useState(1)
  const [formData, setFormData] = useState<TaskCreationFormData>({
    taskName: '',
    taskType: 'ModelGeneration',
    siteId: '',
    priority: 'Normal',
    description: '',
    parameters: {}
  })
  const [selectedTaskTypes, setSelectedTaskTypes] = useState<TaskType[]>(['ModelGeneration'])
  const [parseMode, setParseMode] = useState<ParseMode>("all")
  const [dbnum, setDbnum] = useState<string>("")
  const [refno, setRefno] = useState<string>("")
  const [loading, setLoading] = useState(false)
  const [result, setResult] = useState<{ type: "success" | "error"; message: string } | null>(null)
  const [taskCreated, setTaskCreated] = useState(false)
  const [createdTaskId, setCreatedTaskId] = useState<string | null>(null)

  const {
    sites,
    templates,
    loading: apiLoading,
    error,
    loadSites,
    loadTemplates,
    validateName,
    previewConfig,
    submitTask,
    setError
  } = useTaskCreation()

  useEffect(() => {
    loadSites()
    loadTemplates()
  }, [loadSites, loadTemplates])

  const toggleTaskType = (taskType: TaskType) => {
    setSelectedTaskTypes(prev => {
      if (prev.includes(taskType)) {
        return prev.filter(t => t !== taskType)
      }
      return [...prev, taskType]
    })
  }

  const updateFormData = (updates: Partial<TaskCreationFormData>) => {
    setFormData(prev => ({ ...prev, ...updates }))
  }

  const handleQuickSubmit = async (e: React.FormEvent) => {
    e.preventDefault()

    if (!formData.siteId) {
      setResult({ type: "error", message: "请选择一个站点" })
      return
    }

    if (selectedTaskTypes.length === 0) {
      setResult({ type: "error", message: "请至少选择一个任务类型" })
      return
    }

    // 验证输入
    if (parseMode === "dbnum" && !dbnum) {
      setResult({ type: "error", message: "请输入数据库编号" })
      return
    }
    if (parseMode === "refno" && !refno) {
      setResult({ type: "error", message: "请输入参考号" })
      return
    }

    setLoading(true)
    setResult(null)

    try {
      // 为每个任务类型创建独立的任务
      const taskPromises = selectedTaskTypes.map(async (taskType) => {
        const config: ParseTaskConfig = {
          site_id: formData.siteId,
          task_types: [taskType],
          parse_mode: parseMode,
          priority: formData.priority,
          task_name: formData.taskName || undefined,
        }

        if (parseMode === "dbnum" && dbnum) {
          config.dbnum = parseInt(dbnum)
        }
        if (parseMode === "refno" && refno) {
          config.refno = refno
        }

        // 构建任务创建请求
        const taskRequest = {
          taskName: formData.taskName || `${getTaskTypeName(taskType)}_${Date.now()}`,
          taskType: taskType,
          siteId: formData.siteId,
          priority: formData.priority,
          description: formData.description,
          parameters: {
            parseMode: parseMode,
            dbnum: config.dbnum,
            refno: config.refno,
            maxConcurrent: 1,
            parallelProcessing: false
          }
        }

        return await submitTask(taskRequest)
      })

      const results = await Promise.all(taskPromises)
      const successCount = results.filter(r => r.success).length

      setResult({
        type: "success",
        message: `成功创建 ${successCount} 个任务！任务类型: ${selectedTaskTypes.map(t => getTaskTypeName(t)).join("、")}`
      })

      setTaskCreated(true)
      setCreatedTaskId(results[0]?.taskId || null)

      // 重置表单
      setTimeout(() => {
        setFormData({
          taskName: '',
          taskType: 'ModelGeneration',
          siteId: '',
          priority: 'Normal',
          description: '',
          parameters: {}
        })
        setSelectedTaskTypes(['ModelGeneration'])
        setParseMode("all")
        setDbnum("")
        setRefno("")
        setResult(null)
        setTaskCreated(false)
        setCreatedTaskId(null)
      }, 5000)
    } catch (error) {
      setResult({
        type: "error",
        message: error instanceof Error ? error.message : "创建任务失败"
      })
    } finally {
      setLoading(false)
    }
  }

  const handleAdvancedSubmit = async () => {
    try {
      const result = await submitTask(formData)
      setTaskCreated(true)
      setCreatedTaskId(result.taskId)
      
      // 3秒后跳转到任务监控页面
      setTimeout(() => {
        router.push('/task-monitor')
      }, 3000)
    } catch (error) {
      setResult({
        type: "error",
        message: error instanceof Error ? error.message : "创建任务失败"
      })
    }
  }

  const getTaskTypeName = (type: TaskType): string => {
    const taskType = TASK_TYPES.find(t => t.value === type)
    return taskType?.label || type
  }

  const getPriorityName = (p: TaskPriority): string => {
    const priority = PRIORITY_OPTIONS.find(opt => opt.value === p)
    return priority?.label || p
  }

  const canProceed = () => {
    if (activeTab === "quick") {
      return formData.siteId && selectedTaskTypes.length > 0
    } else {
      return formData.taskName.trim() !== '' && 
             formData.taskType !== '' && 
             formData.siteId !== ''
    }
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
                <Zap className="h-6 w-6 text-primary" />
              </div>
              <div>
                <h1 className="text-3xl font-bold text-foreground">任务创建向导</h1>
                <p className="text-muted-foreground">快速创建解析和模型生成任务</p>
              </div>
            </div>
          </div>

          {/* 模式选择 */}
          <Tabs value={activeTab} onValueChange={(value) => setActiveTab(value as "quick" | "advanced")} className="mb-8">
            <TabsList className="grid w-full grid-cols-2">
              <TabsTrigger value="quick">快速模式</TabsTrigger>
              <TabsTrigger value="advanced">高级模式</TabsTrigger>
            </TabsList>

            {/* 快速模式 */}
            <TabsContent value="quick" className="space-y-6">
              <form onSubmit={handleQuickSubmit}>
                {/* 步骤1: 选择站点 */}
                <Card className="mb-6">
                  <CardHeader>
                    <div className="flex items-center gap-3">
                      <div className="w-8 h-8 bg-primary rounded-full flex items-center justify-center text-primary-foreground font-bold">
                        1
                      </div>
                      <CardTitle>选择部署站点</CardTitle>
                    </div>
                    <CardDescription>请选择要执行解析任务的部署站点</CardDescription>
                  </CardHeader>
                  <CardContent>
                    {apiLoading ? (
                      <div className="flex items-center justify-center py-8">
                        <Loader2 className="h-6 w-6 animate-spin mr-2" />
                        <span>加载站点中...</span>
                      </div>
                    ) : (
                      <RadioGroup value={formData.siteId} onValueChange={(value) => updateFormData({ siteId: value })}>
                        <div className="space-y-3">
                          {sites.map((site) => (
                            <div
                              key={site.id}
                              className={`flex items-center space-x-3 p-4 rounded-lg border-2 transition-colors ${
                                formData.siteId === site.id
                                  ? "border-primary bg-primary/5"
                                  : "border-border hover:border-primary/50"
                              }`}
                            >
                              <RadioGroupItem value={site.id} id={site.id} />
                              <Label htmlFor={site.id} className="flex-1 cursor-pointer">
                                <div className="flex items-center gap-3">
                                  <Database className="h-5 w-5 text-primary" />
                                  <div>
                                    <div className="font-semibold">{site.name}</div>
                                    <div className="text-sm text-muted-foreground">
                                      状态: {site.status} · 环境: {site.environment}
                                    </div>
                                    {site.description && (
                                      <div className="text-sm text-muted-foreground mt-1">
                                        {site.description}
                                      </div>
                                    )}
                                  </div>
                                </div>
                              </Label>
                            </div>
                          ))}
                        </div>
                      </RadioGroup>
                    )}
                    {sites.length === 0 && !apiLoading && (
                      <div className="text-center py-8 text-muted-foreground">
                        暂无可用的部署站点
                      </div>
                    )}
                  </CardContent>
                </Card>

                {/* 步骤2: 配置任务 */}
                <Card className="mb-6">
                  <CardHeader>
                    <div className="flex items-center gap-3">
                      <div className="w-8 h-8 bg-primary rounded-full flex items-center justify-center text-primary-foreground font-bold">
                        2
                      </div>
                      <CardTitle>配置解析任务</CardTitle>
                    </div>
                    <CardDescription>选择任务类型和解析范围</CardDescription>
                  </CardHeader>
                  <CardContent className="space-y-6">
                    {/* 任务类型 - 多选 */}
                    <div className="space-y-3">
                      <Label>任务类型 (可多选)</Label>
                      <div className="space-y-3">
                        {TASK_TYPES.map((task) => (
                          <div
                            key={task.value}
                            className={`flex items-center space-x-3 p-3 rounded-lg border transition-colors ${
                              selectedTaskTypes.includes(task.value)
                                ? "border-primary bg-primary/5"
                                : "border-border hover:border-primary/50"
                            }`}
                          >
                            <Checkbox
                              id={task.value}
                              checked={selectedTaskTypes.includes(task.value)}
                              onCheckedChange={() => toggleTaskType(task.value)}
                            />
                            <Label htmlFor={task.value} className="flex-1 cursor-pointer font-normal">
                              <div className="flex items-center gap-2">
                                {task.icon}
                                <span>{task.label}</span>
                              </div>
                              <p className="text-sm text-muted-foreground mt-1">{task.description}</p>
                            </Label>
                          </div>
                        ))}
                      </div>
                      {selectedTaskTypes.length > 0 && (
                        <div className="text-sm text-muted-foreground">
                          已选择 {selectedTaskTypes.length} 个任务类型
                        </div>
                      )}
                    </div>

                    {/* 解析模式 */}
                    <div className="space-y-2">
                      <Label>解析范围</Label>
                      <RadioGroup value={parseMode} onValueChange={(v) => setParseMode(v as ParseMode)}>
                        <div className="space-y-2">
                          <div className="flex items-center space-x-2">
                            <RadioGroupItem value="all" id="all" />
                            <Label htmlFor="all" className="cursor-pointer">全部解析</Label>
                          </div>
                          <div className="flex items-center space-x-2">
                            <RadioGroupItem value="dbnum" id="dbnum" />
                            <Label htmlFor="dbnum" className="cursor-pointer">指定数据库编号 (dbnum)</Label>
                          </div>
                          <div className="flex items-center space-x-2">
                            <RadioGroupItem value="refno" id="refno" />
                            <Label htmlFor="refno" className="cursor-pointer">指定参考号 (refno)</Label>
                          </div>
                        </div>
                      </RadioGroup>
                    </div>

                    {/* 条件输入 */}
                    {parseMode === "dbnum" && (
                      <div className="space-y-2">
                        <Label htmlFor="dbnum-input">数据库编号</Label>
                        <Input
                          id="dbnum-input"
                          type="number"
                          placeholder="例如: 1112"
                          value={dbnum}
                          onChange={(e) => setDbnum(e.target.value)}
                        />
                      </div>
                    )}

                    {parseMode === "refno" && (
                      <div className="space-y-2">
                        <Label htmlFor="refno-input">参考号</Label>
                        <Input
                          id="refno-input"
                          type="text"
                          placeholder="例如: REF-001"
                          value={refno}
                          onChange={(e) => setRefno(e.target.value)}
                        />
                      </div>
                    )}

                    {/* 任务优先级 */}
                    <div className="space-y-2">
                      <Label>任务优先级</Label>
                      <Select value={formData.priority} onValueChange={(value) => updateFormData({ priority: value as TaskPriority })}>
                        <SelectTrigger>
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {PRIORITY_OPTIONS.map((option) => (
                            <SelectItem key={option.value} value={option.value}>
                              <div className="flex items-center gap-2">
                                <Badge className={option.color}>{option.label}</Badge>
                              </div>
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>

                    {/* 任务名称 (可选) */}
                    <div className="space-y-2">
                      <Label htmlFor="task-name">任务名称 (可选)</Label>
                      <Input
                        id="task-name"
                        type="text"
                        placeholder="留空则自动生成"
                        value={formData.taskName}
                        onChange={(e) => updateFormData({ taskName: e.target.value })}
                      />
                    </div>
                  </CardContent>
                </Card>

                {/* 结果提示 */}
                {result && (
                  <Alert className={`mb-6 ${result.type === "success" ? "border-green-500" : "border-red-500"}`}>
                    {result.type === "success" ? (
                      <CheckCircle2 className="h-4 w-4 text-green-600" />
                    ) : (
                      <AlertCircle className="h-4 w-4 text-red-600" />
                    )}
                    <AlertDescription className={result.type === "success" ? "text-green-600" : "text-red-600"}>
                      {result.message}
                    </AlertDescription>
                  </Alert>
                )}

                {/* 提交按钮 */}
                <div className="flex justify-end gap-3">
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() => {
                      updateFormData({
                        taskName: '',
                        siteId: '',
                        priority: 'Normal',
                        description: '',
                        parameters: {}
                      })
                      setSelectedTaskTypes(['ModelGeneration'])
                      setParseMode("all")
                      setDbnum("")
                      setRefno("")
                      setResult(null)
                    }}
                  >
                    重置
                  </Button>
                  <Button type="submit" disabled={loading || !canProceed()}>
                    {loading ? (
                      <>
                        <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                        创建中...
                      </>
                    ) : (
                      <>
                        <ChevronRight className="h-4 w-4 mr-2" />
                        创建任务
                      </>
                    )}
                  </Button>
                </div>
              </form>
            </TabsContent>

            {/* 高级模式 */}
            <TabsContent value="advanced" className="space-y-6">
              <div className="text-center py-8">
                <div className="w-16 h-16 bg-primary/10 rounded-full flex items-center justify-center mx-auto mb-4">
                  <Settings className="h-8 w-8 text-primary" />
                </div>
                <h3 className="text-xl font-semibold mb-2">高级模式</h3>
                <p className="text-muted-foreground mb-6">
                  使用高级模式可以获得更精细的任务配置选项
                </p>
                <Button onClick={() => router.push('/task-creation')}>
                  <ArrowRight className="h-4 w-4 mr-2" />
                  前往高级任务创建
                </Button>
              </div>
            </TabsContent>
          </Tabs>

          {/* 任务创建成功对话框 */}
          {taskCreated && (
            <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
              <Card className="w-96">
                <CardContent className="p-6">
                  <div className="text-center">
                    <CheckCircle2 className="h-12 w-12 text-green-600 mx-auto mb-4" />
                    <h3 className="text-lg font-semibold mb-2">任务创建成功</h3>
                    <p className="text-sm text-muted-foreground mb-4">
                      任务ID: <code className="bg-muted px-2 py-1 rounded">{createdTaskId}</code>
                    </p>
                    <p className="text-sm text-muted-foreground mb-4">
                      正在跳转到任务监控页面...
                    </p>
                    <Button onClick={() => router.push('/task-monitor')} className="w-full">
                      立即跳转
                    </Button>
                  </div>
                </CardContent>
              </Card>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}