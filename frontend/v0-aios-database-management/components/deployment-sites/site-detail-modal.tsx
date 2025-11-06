"use client"

import { useState, useEffect } from "react"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { 
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog"
import { 
  Database, 
  Settings, 
  Play, 
  Pause, 
  Square, 
  RefreshCw, 
  AlertCircle,
  Loader2,
  ExternalLink,
  Copy,
  Edit,
  Trash2,
  CheckCircle,
  XCircle,
  Clock,
  FileText,
  Layers,
  TreePine
} from "lucide-react"
import { toast } from "sonner"
import { formatDistanceToNow } from "date-fns"
import { zhCN } from "date-fns/locale"
import type { Site } from "./site-card"
import type { DeploymentSiteConfigPayload } from "@/lib/api"
import { fetchDeploymentSite } from "@/lib/api"

interface SiteDetail extends Site {
  config: DeploymentSiteConfigPayload
  description?: string
  rootDirectory?: string
  selectedProjects: string[]
  tags?: Record<string, unknown>
  notes?: string
}

interface SiteDetailModalProps {
  site: Site | null
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function SiteDetailModal({ site, open, onOpenChange }: SiteDetailModalProps) {
  const [siteDetail, setSiteDetail] = useState<SiteDetail | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [dbStatus, setDbStatus] = useState<"unknown" | "starting" | "running" | "stopped">("unknown")
  const [isDbLoading, setIsDbLoading] = useState(false)
  const [parsingStatus, setParsingStatus] = useState<"unknown" | "parsing" | "completed" | "failed">("unknown")
  const [modelGenerationStatus, setModelGenerationStatus] = useState<"unknown" | "generating" | "completed" | "failed">("unknown")

  // 加载站点详情
  useEffect(() => {
    if (!site || !open) return

    const loadSiteDetail = async () => {
      try {
        setLoading(true)
        setError(null)
        
        // 调用API获取站点详情
        const response = await fetchDeploymentSite(site.id)
        const siteData = response.item
        
        if (!siteData) {
          throw new Error("站点不存在")
        }
        
        // 转换API数据为组件需要的格式
        const detail: SiteDetail = {
          id: siteData.id as string,
          name: siteData.name as string,
          status: ((siteData.status as string) || "configuring") as Site["status"],
          environment: ((siteData.env as string) || "dev") as Site["environment"],
          owner: siteData.owner as string,
          createdAt: siteData.created_at as string || new Date().toISOString(),
          updatedAt: siteData.updated_at as string || new Date().toISOString(),
          description: siteData.description as string,
          url: siteData.url as string,
          rootDirectory: siteData.root_directory as string,
          selectedProjects: (siteData.selected_projects as string[]) || [],
          config: siteData.config as DeploymentSiteConfigPayload,
          tags: siteData.tags as Record<string, unknown>,
          notes: siteData.notes as string,
        }
        
        setSiteDetail(detail)
        
        // 加载状态信息
        await Promise.all([
          checkDbStatus(detail),
          checkParsingStatus(detail),
          checkModelGenerationStatus(detail)
        ])
      } catch (err) {
        setError(err instanceof Error ? err.message : "加载站点详情失败")
      } finally {
        setLoading(false)
      }
    }

    loadSiteDetail()
  }, [site, open])

  // 检查数据库状态
  const checkDbStatus = async (siteData?: SiteDetail) => {
    const targetSite = siteData || siteDetail
    if (!targetSite) return
    
    setIsDbLoading(true)
    try {
      const baseUrl = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "") || ""
      const response = await fetch(
        `${baseUrl}/api/database/startup/status?ip=${targetSite.config.db_ip}&port=${targetSite.config.db_port}`
      )
      const data = await response.json()

      if (data.success) {
        const status = data.status
        if (status === "Running") {
          setDbStatus("running")
        } else if (status === "Starting") {
          setDbStatus("starting")
        } else {
          setDbStatus("stopped")
        }
      } else {
        setDbStatus("stopped")
      }
    } catch (err) {
      console.error("检查数据库状态失败:", err)
      setDbStatus("stopped")
    } finally {
      setIsDbLoading(false)
    }
  }

  // 检查解析状态
  const checkParsingStatus = async (siteData?: SiteDetail) => {
    const targetSite = siteData || siteDetail
    if (!targetSite) return

    try {
      // 这里应该调用实际的解析状态API
      // 暂时模拟状态
      const baseUrl = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "") || ""
      const response = await fetch(
        `${baseUrl}/api/parsing/status?siteId=${targetSite.id}`
      )
      
      if (response.ok) {
        const data = await response.json()
        setParsingStatus(data.status || "unknown")
      } else {
        setParsingStatus("unknown")
      }
    } catch (err) {
      console.error("检查解析状态失败:", err)
      setParsingStatus("unknown")
    }
  }

  // 检查模型生成状态
  const checkModelGenerationStatus = async (siteData?: SiteDetail) => {
    const targetSite = siteData || siteDetail
    if (!targetSite) return

    try {
      // 这里应该调用实际的模型生成状态API
      // 暂时模拟状态
      const baseUrl = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "") || ""
      const response = await fetch(
        `${baseUrl}/api/model-generation/status?siteId=${targetSite.id}`
      )
      
      if (response.ok) {
        const data = await response.json()
        setModelGenerationStatus(data.status || "unknown")
      } else {
        setModelGenerationStatus("unknown")
      }
    } catch (err) {
      console.error("检查模型生成状态失败:", err)
      setModelGenerationStatus("unknown")
    }
  }

  // 启动数据库
  const startDatabase = async () => {
    if (!siteDetail) return
    
    setIsDbLoading(true)
    try {
      const baseUrl = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "") || ""
      const response = await fetch(`${baseUrl}/api/database/startup/start`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          ip: siteDetail.config.db_ip,
          port: parseInt(siteDetail.config.db_port),
          user: siteDetail.config.db_user,
          password: siteDetail.config.db_password,
          dbFile: "surreal.db"
        })
      })
      const data = await response.json()

      if (data.success) {
        setDbStatus("starting")
        toast.success("数据库启动请求已提交")
        // 轮询检查状态
        setTimeout(() => checkDbStatus(), 2000)
      } else {
        toast.error(data.error || "启动数据库失败")
      }
    } catch (err) {
      toast.error("启动数据库时发生错误")
    } finally {
      setIsDbLoading(false)
    }
  }

  // 停止数据库
  const stopDatabase = async () => {
    if (!siteDetail) return
    
    setIsDbLoading(true)
    try {
      const baseUrl = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "") || ""
      const response = await fetch(`${baseUrl}/api/database/startup/stop`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          ip: siteDetail.config.db_ip,
          port: parseInt(siteDetail.config.db_port)
        })
      })
      const data = await response.json()

      if (data.success) {
        setDbStatus("stopped")
        toast.success("数据库已停止")
      } else {
        toast.error(data.error || "停止数据库失败")
      }
    } catch (err) {
      toast.error("停止数据库时发生错误")
    } finally {
      setIsDbLoading(false)
    }
  }

  const handleCopyUrl = () => {
    if (siteDetail?.url) {
      navigator.clipboard.writeText(siteDetail.url)
      toast.success("访问地址已复制到剪贴板")
    }
  }

  const statusConfig = {
    running: { label: "运行中", color: "bg-green-100 text-green-800" },
    deploying: { label: "部署中", color: "bg-blue-100 text-blue-800" },
    configuring: { label: "配置中", color: "bg-orange-100 text-orange-800" },
    failed: { label: "失败", color: "bg-red-100 text-red-800" },
    paused: { label: "已暂停", color: "bg-gray-100 text-gray-800" },
    stopped: { label: "已停止", color: "bg-gray-200 text-gray-900" },
  }

  const environmentConfig = {
    dev: { label: "开发", color: "bg-yellow-100 text-yellow-800" },
    test: { label: "测试", color: "bg-purple-100 text-purple-800" },
    staging: { label: "预发布", color: "bg-orange-100 text-orange-800" },
    prod: { label: "生产", color: "bg-red-100 text-red-800" },
  }

  const getStatusIcon = (status: string) => {
    switch (status) {
      case "running":
      case "completed":
        return <CheckCircle className="h-4 w-4 text-green-600" />
      case "starting":
      case "parsing":
      case "generating":
        return <Clock className="h-4 w-4 text-blue-600" />
      case "stopped":
      case "failed":
        return <XCircle className="h-4 w-4 text-red-600" />
      default:
        return <AlertCircle className="h-4 w-4 text-gray-600" />
    }
  }

  const getStatusText = (status: string, type: "db" | "parsing" | "model") => {
    const statusMap = {
      db: {
        running: "数据库运行中",
        starting: "数据库启动中",
        stopped: "数据库已停止",
        unknown: "数据库状态未知"
      },
      parsing: {
        completed: "数据解析完成",
        parsing: "数据解析中",
        failed: "数据解析失败",
        unknown: "解析状态未知"
      },
      model: {
        completed: "模型生成完成",
        generating: "模型生成中",
        failed: "模型生成失败",
        unknown: "生成状态未知"
      }
    }
    return statusMap[type][status as keyof typeof statusMap[typeof type]] || "状态未知"
  }

  if (!site) return null

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-4xl max-h-[90vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle className="flex items-center space-x-3">
            <Database className="h-6 w-6" />
            <span>{site.name}</span>
            <Badge variant="secondary" className={statusConfig[site.status].color}>
              {statusConfig[site.status].label}
            </Badge>
            <Badge variant="outline" className={environmentConfig[site.environment].color}>
              {environmentConfig[site.environment].label}
            </Badge>
          </DialogTitle>
          <DialogDescription>
            查看站点的详细配置信息和运行状态
          </DialogDescription>
        </DialogHeader>

        {loading ? (
          <div className="flex items-center justify-center py-8">
            <Loader2 className="h-8 w-8 animate-spin" />
            <span className="ml-2">加载站点详情中...</span>
          </div>
        ) : error ? (
          <Alert variant="destructive">
            <AlertCircle className="h-4 w-4" />
            <AlertTitle>加载失败</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        ) : siteDetail ? (
          <div className="space-y-6">
            {/* 状态概览 */}
            <Card>
              <CardHeader>
                <CardTitle className="flex items-center space-x-2">
                  <Settings className="h-5 w-5" />
                  运行状态概览
                </CardTitle>
              </CardHeader>
              <CardContent>
                <div className="grid grid-cols-3 gap-4">
                  <div className="flex items-center space-x-3 p-3 border rounded-lg">
                    {getStatusIcon(dbStatus)}
                    <div>
                      <p className="text-sm font-medium">数据库状态</p>
                      <p className="text-xs text-muted-foreground">
                        {getStatusText(dbStatus, "db")}
                      </p>
                    </div>
                  </div>
                  <div className="flex items-center space-x-3 p-3 border rounded-lg">
                    {getStatusIcon(parsingStatus)}
                    <div>
                      <p className="text-sm font-medium">解析状态</p>
                      <p className="text-xs text-muted-foreground">
                        {getStatusText(parsingStatus, "parsing")}
                      </p>
                    </div>
                  </div>
                  <div className="flex items-center space-x-3 p-3 border rounded-lg">
                    {getStatusIcon(modelGenerationStatus)}
                    <div>
                      <p className="text-sm font-medium">模型生成</p>
                      <p className="text-xs text-muted-foreground">
                        {getStatusText(modelGenerationStatus, "model")}
                      </p>
                    </div>
                  </div>
                </div>
              </CardContent>
            </Card>

            {/* 基本信息 */}
            <Card>
              <CardHeader>
                <CardTitle className="flex items-center space-x-2">
                  <Settings className="h-5 w-5" />
                  基本信息
                </CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <div className="grid grid-cols-2 gap-4">
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">站点名称</label>
                    <p className="text-sm">{siteDetail.name}</p>
                  </div>
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">负责人</label>
                    <p className="text-sm">{siteDetail.owner || "未指定"}</p>
                  </div>
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">创建时间</label>
                    <p className="text-sm">
                      {formatDistanceToNow(new Date(siteDetail.createdAt), {
                        addSuffix: true,
                        locale: zhCN,
                      })}
                    </p>
                  </div>
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">更新时间</label>
                    <p className="text-sm">
                      {formatDistanceToNow(new Date(siteDetail.updatedAt), {
                        addSuffix: true,
                        locale: zhCN,
                      })}
                    </p>
                  </div>
                </div>
                
                {siteDetail.description && (
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">描述</label>
                    <p className="text-sm">{siteDetail.description}</p>
                  </div>
                )}

                {siteDetail.url && (
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">访问地址</label>
                    <div className="flex items-center space-x-2">
                      <a
                        href={siteDetail.url}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="text-primary hover:underline flex items-center space-x-1"
                      >
                        <span>{siteDetail.url}</span>
                        <ExternalLink className="h-3 w-3" />
                      </a>
                      <Button variant="ghost" size="sm" onClick={handleCopyUrl} className="h-6 w-6 p-0">
                        <Copy className="h-3 w-3" />
                      </Button>
                    </div>
                  </div>
                )}
              </CardContent>
            </Card>

            {/* 数据库配置 */}
            <Card>
              <CardHeader>
                <CardTitle className="flex items-center space-x-2">
                  <Database className="h-5 w-5" />
                  数据库配置
                </CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <div className="grid grid-cols-2 gap-4">
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">数据库类型</label>
                    <p className="text-sm font-mono">{siteDetail.config.db_type.toUpperCase()}</p>
                  </div>
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">项目代码</label>
                    <p className="text-sm font-mono">{siteDetail.config.project_code}</p>
                  </div>
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">数据库 IP</label>
                    <p className="text-sm font-mono">{siteDetail.config.db_ip}</p>
                  </div>
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">端口</label>
                    <p className="text-sm font-mono">{siteDetail.config.db_port}</p>
                  </div>
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">用户名</label>
                    <p className="text-sm font-mono">{siteDetail.config.db_user}</p>
                  </div>
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">密码</label>
                    <p className="text-sm font-mono">••••••••</p>
                  </div>
                </div>

                {/* Database Controls */}
                <div className="border-t pt-4">
                  <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-2">
                      <label className="text-sm font-medium">数据库控制</label>
                      {dbStatus !== "unknown" && (
                        <span className={`text-xs ${dbStatus === "running" ? "text-green-600" : "text-muted-foreground"}`}>
                          {dbStatus === "running" ? "● 运行中" : dbStatus === "starting" ? "○ 启动中" : "○ 已停止"}
                        </span>
                      )}
                    </div>
                    <div className="flex items-center space-x-2">
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => checkDbStatus()}
                        disabled={isDbLoading}
                      >
                        {isDbLoading && <Loader2 className="h-3 w-3 mr-1 animate-spin" />}
                        检查状态
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={startDatabase}
                        disabled={isDbLoading || dbStatus === "running"}
                      >
                        <Play className="h-3 w-3 mr-1" />
                        启动数据库
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={stopDatabase}
                        disabled={isDbLoading || dbStatus === "stopped"}
                      >
                        <Square className="h-3 w-3 mr-1" />
                        停止数据库
                      </Button>
                    </div>
                  </div>
                </div>
              </CardContent>
            </Card>

            {/* 项目配置 */}
            <Card>
              <CardHeader>
                <CardTitle className="flex items-center space-x-2">
                  <FileText className="h-5 w-5" />
                  项目配置
                </CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <div className="grid grid-cols-2 gap-4">
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">项目名称</label>
                    <p className="text-sm font-mono">{siteDetail.config.project_name}</p>
                  </div>
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">项目路径</label>
                    <p className="text-sm font-mono">{siteDetail.config.project_path}</p>
                  </div>
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">MDB 名称</label>
                    <p className="text-sm font-mono">{siteDetail.config.mdb_name}</p>
                  </div>
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">模块</label>
                    <p className="text-sm font-mono">{siteDetail.config.module}</p>
                  </div>
                </div>

                {siteDetail.selectedProjects.length > 0 && (
                  <div>
                    <label className="text-sm font-medium text-muted-foreground">已选项目</label>
                    <div className="flex flex-wrap gap-2 mt-2">
                      {siteDetail.selectedProjects.map((project, index) => (
                        <Badge key={index} variant="outline">
                          {project}
                        </Badge>
                      ))}
                    </div>
                  </div>
                )}
              </CardContent>
            </Card>

            {/* 生成选项 */}
            <Card>
              <CardHeader>
                <CardTitle className="flex items-center space-x-2">
                  <Layers className="h-5 w-5" />
                  生成选项
                </CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <div className="grid grid-cols-2 gap-4">
                  <div className="space-y-2">
                    <label className="text-sm font-medium text-muted-foreground">生成选项</label>
                    <div className="space-y-1">
                      <div className="flex items-center space-x-2">
                        <div className={`w-2 h-2 rounded-full ${siteDetail.config.gen_model ? 'bg-green-500' : 'bg-gray-300'}`} />
                        <span className="text-sm">生成 3D 模型</span>
                      </div>
                      <div className="flex items-center space-x-2">
                        <div className={`w-2 h-2 rounded-full ${siteDetail.config.gen_mesh ? 'bg-green-500' : 'bg-gray-300'}`} />
                        <span className="text-sm">生成网格数据</span>
                      </div>
                      <div className="flex items-center space-x-2">
                        <div className={`w-2 h-2 rounded-full ${siteDetail.config.gen_spatial_tree ? 'bg-green-500' : 'bg-gray-300'}`} />
                        <span className="text-sm">生成空间树</span>
                      </div>
                      <div className="flex items-center space-x-2">
                        <div className={`w-2 h-2 rounded-full ${siteDetail.config.apply_boolean_operation ? 'bg-green-500' : 'bg-gray-300'}`} />
                        <span className="text-sm">应用布尔运算</span>
                      </div>
                    </div>
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium text-muted-foreground">高级设置</label>
                    <div className="space-y-1">
                      <div>
                        <span className="text-sm text-muted-foreground">网格容差比率: </span>
                        <span className="text-sm font-mono">{siteDetail.config.mesh_tol_ratio}</span>
                      </div>
                      <div>
                        <span className="text-sm text-muted-foreground">房间关键字: </span>
                        <span className="text-sm font-mono">{siteDetail.config.room_keyword}</span>
                      </div>
                      {siteDetail.config.target_sesno && (
                        <div>
                          <span className="text-sm text-muted-foreground">目标会话号: </span>
                          <span className="text-sm font-mono">{siteDetail.config.target_sesno}</span>
                        </div>
                      )}
                    </div>
                  </div>
                </div>
              </CardContent>
            </Card>

            {/* 操作按钮 */}
            <div className="flex items-center justify-end space-x-2 pt-4 border-t">
              <Button variant="outline" size="sm">
                <Edit className="h-4 w-4 mr-2" />
                编辑配置
              </Button>
              <Button variant="outline" size="sm" className="text-red-600">
                <Trash2 className="h-4 w-4 mr-2" />
                删除站点
              </Button>
            </div>
          </div>
        ) : null}
      </DialogContent>
    </Dialog>
  )
}





