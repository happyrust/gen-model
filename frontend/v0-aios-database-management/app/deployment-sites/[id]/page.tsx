"use client"

/**
 * 部署站点详情页面
 * 
 * 显示站点的详细配置信息，包括：
 * - 基本信息（名称、描述、状态等）
 * - 数据库配置
 * - 项目配置
 * - 生成选项
 * - 操作历史
 */

import { useState, useEffect } from "react"
import { useParams, useRouter } from "next/navigation"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { 
  ArrowLeft, 
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
  Trash2
} from "lucide-react"
import { Sidebar } from "@/components/sidebar"
import { toast } from "sonner"
import { formatDistanceToNow } from "date-fns"
import { zhCN } from "date-fns/locale"
import type { Site } from "@/components/deployment-sites/site-card"
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

export default function SiteDetailPage() {
  const params = useParams()
  const router = useRouter()
  const siteId = params.id as string

  const [site, setSite] = useState<SiteDetail | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [dbStatus, setDbStatus] = useState<"unknown" | "starting" | "running" | "stopped">("unknown")
  const [isDbLoading, setIsDbLoading] = useState(false)

  // 加载站点详情
  useEffect(() => {
    const loadSiteDetail = async () => {
      try {
        setLoading(true)
        setError(null)
        
        // 调用API获取站点详情
        const response = await fetchDeploymentSite(siteId)
        const siteData = response.item
        
        if (!siteData) {
          throw new Error("站点不存在")
        }
        
        // 转换API数据为组件需要的格式
        const siteDetail: SiteDetail = {
          id: siteData.id as string,
          name: siteData.name as string,
          status: (siteData.status as string) || "configuring",
          environment: (siteData.env as string) || "dev",
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
        
        setSite(siteDetail)
      } catch (err) {
        setError(err instanceof Error ? err.message : "加载站点详情失败")
      } finally {
        setLoading(false)
      }
    }

    if (siteId) {
      loadSiteDetail()
    }
  }, [siteId])

  // 检查数据库状态
  const checkDbStatus = async () => {
    if (!site) return
    
    setIsDbLoading(true)
    try {
      const baseUrl = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "") || ""
      const response = await fetch(
        `${baseUrl}/api/database/startup/status?ip=${site.config.db_ip}&port=${site.config.db_port}`
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

  // 启动数据库
  const startDatabase = async () => {
    if (!site) return
    
    setIsDbLoading(true)
    try {
      const baseUrl = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "") || ""
      const response = await fetch(`${baseUrl}/api/database/startup/start`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          ip: site.config.db_ip,
          port: parseInt(site.config.db_port),
          user: site.config.db_user,
          password: site.config.db_password,
          dbFile: "surreal.db"
        })
      })
      const data = await response.json()

      if (data.success) {
        setDbStatus("starting")
        toast.success("数据库启动请求已提交")
        // 轮询检查状态
        setTimeout(checkDbStatus, 2000)
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
    if (!site) return
    
    setIsDbLoading(true)
    try {
      const baseUrl = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "") || ""
      const response = await fetch(`${baseUrl}/api/database/startup/stop`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          ip: site.config.db_ip,
          port: parseInt(site.config.db_port)
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
    if (site?.url) {
      navigator.clipboard.writeText(site.url)
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

  if (loading) {
    return (
      <div className="min-h-screen bg-background">
        <Sidebar />
        <div className="ml-64 p-8">
          <div className="flex items-center justify-center h-64">
            <Loader2 className="h-8 w-8 animate-spin" />
            <span className="ml-2">加载站点详情中...</span>
          </div>
        </div>
      </div>
    )
  }

  if (error || !site) {
    return (
      <div className="min-h-screen bg-background">
        <Sidebar />
        <div className="ml-64 p-8">
          <Alert variant="destructive">
            <AlertCircle className="h-4 w-4" />
            <AlertTitle>加载失败</AlertTitle>
            <AlertDescription>{error || "站点不存在"}</AlertDescription>
          </Alert>
          <Button onClick={() => router.back()} className="mt-4">
            <ArrowLeft className="h-4 w-4 mr-2" />
            返回
          </Button>
        </div>
      </div>
    )
  }

  const statusInfo = statusConfig[site.status]
  const envInfo = environmentConfig[site.environment]

  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      
      <div className="ml-64 p-8">
        <div className="space-y-6">
          {/* Header */}
          <div className="flex items-center justify-between">
            <div className="flex items-center space-x-4">
              <Button variant="outline" size="sm" onClick={() => router.back()}>
                <ArrowLeft className="h-4 w-4 mr-2" />
                返回
              </Button>
              <div className="flex items-center space-x-3">
                <Database className="h-6 w-6" />
                <h1 className="text-2xl font-bold">{site.name}</h1>
                <Badge variant="secondary" className={statusInfo.color}>
                  {statusInfo.label}
                </Badge>
                <Badge variant="outline" className={envInfo.color}>
                  {envInfo.label}
                </Badge>
              </div>
            </div>
            
            <div className="flex items-center space-x-2">
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

          {/* Basic Info */}
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
                  <p className="text-sm">{site.name}</p>
                </div>
                <div>
                  <label className="text-sm font-medium text-muted-foreground">负责人</label>
                  <p className="text-sm">{site.owner || "未指定"}</p>
                </div>
                <div>
                  <label className="text-sm font-medium text-muted-foreground">创建时间</label>
                  <p className="text-sm">
                    {formatDistanceToNow(new Date(site.createdAt), {
                      addSuffix: true,
                      locale: zhCN,
                    })}
                  </p>
                </div>
                <div>
                  <label className="text-sm font-medium text-muted-foreground">更新时间</label>
                  <p className="text-sm">
                    {formatDistanceToNow(new Date(site.updatedAt), {
                      addSuffix: true,
                      locale: zhCN,
                    })}
                  </p>
                </div>
              </div>
              
              {site.description && (
                <div>
                  <label className="text-sm font-medium text-muted-foreground">描述</label>
                  <p className="text-sm">{site.description}</p>
                </div>
              )}

              {site.url && (
                <div>
                  <label className="text-sm font-medium text-muted-foreground">访问地址</label>
                  <div className="flex items-center space-x-2">
                    <a
                      href={site.url}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="text-primary hover:underline flex items-center space-x-1"
                    >
                      <span>{site.url}</span>
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

          {/* Database Configuration */}
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
                  <p className="text-sm font-mono">{site.config.db_type.toUpperCase()}</p>
                </div>
                <div>
                  <label className="text-sm font-medium text-muted-foreground">项目代码</label>
                  <p className="text-sm font-mono">{site.config.project_code}</p>
                </div>
                <div>
                  <label className="text-sm font-medium text-muted-foreground">数据库 IP</label>
                  <p className="text-sm font-mono">{site.config.db_ip}</p>
                </div>
                <div>
                  <label className="text-sm font-medium text-muted-foreground">端口</label>
                  <p className="text-sm font-mono">{site.config.db_port}</p>
                </div>
                <div>
                  <label className="text-sm font-medium text-muted-foreground">用户名</label>
                  <p className="text-sm font-mono">{site.config.db_user}</p>
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
                    <label className="text-sm font-medium">数据库状态</label>
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
                      onClick={checkDbStatus}
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

          {/* Project Configuration */}
          <Card>
            <CardHeader>
              <CardTitle>项目配置</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="grid grid-cols-2 gap-4">
                <div>
                  <label className="text-sm font-medium text-muted-foreground">项目名称</label>
                  <p className="text-sm font-mono">{site.config.project_name}</p>
                </div>
                <div>
                  <label className="text-sm font-medium text-muted-foreground">项目路径</label>
                  <p className="text-sm font-mono">{site.config.project_path}</p>
                </div>
                <div>
                  <label className="text-sm font-medium text-muted-foreground">MDB 名称</label>
                  <p className="text-sm font-mono">{site.config.mdb_name}</p>
                </div>
                <div>
                  <label className="text-sm font-medium text-muted-foreground">模块</label>
                  <p className="text-sm font-mono">{site.config.module}</p>
                </div>
              </div>

              {site.selectedProjects.length > 0 && (
                <div>
                  <label className="text-sm font-medium text-muted-foreground">已选项目</label>
                  <div className="flex flex-wrap gap-2 mt-2">
                    {site.selectedProjects.map((project, index) => (
                      <Badge key={index} variant="outline">
                        {project}
                      </Badge>
                    ))}
                  </div>
                </div>
              )}
            </CardContent>
          </Card>

          {/* Generation Options */}
          <Card>
            <CardHeader>
              <CardTitle>生成选项</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-2">
                  <label className="text-sm font-medium text-muted-foreground">生成选项</label>
                  <div className="space-y-1">
                    <div className="flex items-center space-x-2">
                      <div className={`w-2 h-2 rounded-full ${site.config.gen_model ? 'bg-green-500' : 'bg-gray-300'}`} />
                      <span className="text-sm">生成 3D 模型</span>
                    </div>
                    <div className="flex items-center space-x-2">
                      <div className={`w-2 h-2 rounded-full ${site.config.gen_mesh ? 'bg-green-500' : 'bg-gray-300'}`} />
                      <span className="text-sm">生成网格数据</span>
                    </div>
                    <div className="flex items-center space-x-2">
                      <div className={`w-2 h-2 rounded-full ${site.config.gen_spatial_tree ? 'bg-green-500' : 'bg-gray-300'}`} />
                      <span className="text-sm">生成空间树</span>
                    </div>
                    <div className="flex items-center space-x-2">
                      <div className={`w-2 h-2 rounded-full ${site.config.apply_boolean_operation ? 'bg-green-500' : 'bg-gray-300'}`} />
                      <span className="text-sm">应用布尔运算</span>
                    </div>
                  </div>
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-muted-foreground">高级设置</label>
                  <div className="space-y-1">
                    <div>
                      <span className="text-sm text-muted-foreground">网格容差比率: </span>
                      <span className="text-sm font-mono">{site.config.mesh_tol_ratio}</span>
                    </div>
                    <div>
                      <span className="text-sm text-muted-foreground">房间关键字: </span>
                      <span className="text-sm font-mono">{site.config.room_keyword}</span>
                    </div>
                    {site.config.target_sesno && (
                      <div>
                        <span className="text-sm text-muted-foreground">目标会话号: </span>
                        <span className="text-sm font-mono">{site.config.target_sesno}</span>
                      </div>
                    )}
                  </div>
                </div>
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  )
}
