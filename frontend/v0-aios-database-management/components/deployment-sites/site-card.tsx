"use client"

import { useState } from "react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Database, Play, Plus } from "lucide-react"
import { formatDistanceToNow } from "date-fns"
import { zhCN } from "date-fns/locale"
import { SiteDetailModal } from "./site-detail-modal"
import { useRouter } from "next/navigation"
import { startDatabase as apiStartDatabase, stopDatabase as apiStopDatabase } from "@/lib/database-status"
import { fetchDeploymentSite } from "@/lib/api"
import { useDatabaseStatusPoller, type DbSimpleStatus } from "@/hooks/use-database-status"
import { toast } from "sonner"

export interface Site {
  id: string
  name: string
  status: "running" | "deploying" | "configuring" | "failed" | "paused" | "stopped"
  environment: "dev" | "test" | "staging" | "prod"
  owner?: string
  createdAt: string
  updatedAt: string
  url?: string
  description?: string
  // 新增状态字段
  dbStatus?: "unknown" | "starting" | "running" | "stopped"
  parsingStatus?: "unknown" | "parsing" | "completed" | "failed"
  modelGenerationStatus?: "unknown" | "generating" | "completed" | "failed"
}

interface SiteCardProps {
  site: Site
  onView?: (site: Site) => void
  onStart?: (site: Site) => void
  onPause?: (site: Site) => void
  onConfigure?: (site: Site) => void
  onDelete?: (site: Site) => void
}

const statusConfig = {
  running: {
    label: "运行中",
    color: "bg-green-100 text-green-800",
  },
  deploying: {
    label: "部署中",
    color: "bg-blue-100 text-blue-800",
  },
  configuring: {
    label: "配置中",
    color: "bg-orange-100 text-orange-800",
  },
  failed: {
    label: "失败",
    color: "bg-red-100 text-red-800",
  },
  paused: {
    label: "已暂停",
    color: "bg-gray-100 text-gray-800",
  },
  stopped: {
    label: "已停止",
    color: "bg-gray-200 text-gray-900",
  },
}

const environmentConfig = {
  dev: {
    label: "开发",
    color: "bg-yellow-100 text-yellow-800",
  },
  test: {
    label: "测试",
    color: "bg-purple-100 text-purple-800",
  },
  staging: {
    label: "预发布",
    color: "bg-orange-100 text-orange-800",
  },
  prod: {
    label: "生产",
    color: "bg-red-100 text-red-800",
  },
}

export function SiteCard({ site, onView, onStart, onPause, onConfigure, onDelete }: SiteCardProps) {
  const router = useRouter()
  const [showDetailModal, setShowDetailModal] = useState(false)
  const statusInfo = statusConfig[site.status]
  const envInfo = environmentConfig[site.environment] ?? environmentConfig.dev
  const [dbStatus, setDbStatus] = useState<DbSimpleStatus>(site.dbStatus || "unknown")
  const [isDbBusy, setIsDbBusy] = useState(false)
  const [dbConn, setDbConn] = useState<{ ip: string; port: number; user?: string; password?: string } | null>(null)

  const poller = useDatabaseStatusPoller({
    ip: dbConn?.ip,
    port: dbConn?.port,
    startImmediately: false,
    intervalMs: 1000,
    timeoutMs: 180000,
    onStatus: (s) => setDbStatus(s),
  })

  const handleViewDetails = () => {
    setShowDetailModal(true)
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

  const ensureDbConn = async (): Promise<{ ip: string; port: number; user?: string; password?: string } | null> => {
    if (dbConn) return dbConn
    try {
      const res = await fetchDeploymentSite(site.id)
      const cfg = res.item?.config as any
      if (cfg?.db_ip && cfg?.db_port) {
        const conn = {
          ip: String(cfg.db_ip),
          port: parseInt(String(cfg.db_port), 10),
          user: cfg.db_user ? String(cfg.db_user) : undefined,
          password: cfg.db_password ? String(cfg.db_password) : undefined,
        }
        setDbConn(conn)
        return conn
      }
    } catch (e) {
      toast.error("获取数据库连接信息失败")
    }
    return null
  }

  const handleStartDb = async () => {
    const conn = await ensureDbConn()
    if (!conn) return
    try {
      setIsDbBusy(true)
      setDbStatus("starting")
      await apiStartDatabase({
        ip: conn.ip,
        port: conn.port,
        user: conn.user || "",
        password: conn.password || "",
        dbFile: "surreal.db",
      })
      toast.success("数据库启动请求已提交")
    } catch (_) {
      toast.error("启动数据库失败")
      setDbStatus("stopped")
    } finally {
      setIsDbBusy(false)
    }
  }

  const handleStopDb = async () => {
    const conn = await ensureDbConn()
    if (!conn) return
    try {
      setIsDbBusy(true)
      await apiStopDatabase(conn.ip, conn.port)
      poller.stop()
      setDbStatus("stopped")
      toast.success("数据库已停止")
    } catch (_) {
      toast.error("停止数据库失败")
    } finally {
      setIsDbBusy(false)
    }
  }

  return (
    <Card className="flex flex-col h-full hover:shadow-md transition-shadow">
      <CardHeader className="pb-4">
        <div className="flex items-start justify-between gap-4">
          <div className="flex items-start gap-3 flex-1">
            <div className="text-primary mt-1">
              <Database className="w-5 h-5" />
            </div>
            <div className="flex-1 min-w-0">
              <CardTitle className="text-lg truncate cursor-pointer hover:text-primary" onClick={handleViewDetails}>
                {site.name}
              </CardTitle>
              <p className="text-sm text-muted-foreground mt-1">
                {formatDistanceToNow(new Date(site.updatedAt), { addSuffix: true, locale: zhCN })}
              </p>
            </div>
          </div>
          <Badge variant="secondary" className="whitespace-nowrap">
            {statusInfo.label}
          </Badge>
        </div>
      </CardHeader>

      <CardContent className="flex-1 flex flex-col gap-6">
        <div className="space-y-2">
          <h3 className="text-sm font-semibold text-foreground">详情</h3>
          <div className="space-y-1 text-sm">
            <div className="flex justify-between">
              <span className="text-muted-foreground">环境</span>
              <span className="font-medium text-foreground">{envInfo.label}</span>
            </div>
            <div className="flex justify-between">
              <span className="text-muted-foreground">更新时间</span>
              <span className="font-medium text-foreground">
                {formatDistanceToNow(new Date(site.updatedAt), { addSuffix: true, locale: zhCN })}
              </span>
            </div>
          </div>
        </div>

        <div className="space-y-2">
          <h3 className="text-sm font-semibold text-foreground">状态</h3>
          <div className="grid grid-cols-3 gap-2">
            <div className="text-center p-2 bg-muted rounded-md">
              <p className="text-xs text-muted-foreground mb-1">数据库状态</p>
              <p className="text-xs font-medium text-foreground">{getStatusText(site.dbStatus || "unknown", "db")}</p>
            </div>
            <div className="text-center p-2 bg-muted rounded-md">
              <p className="text-xs text-muted-foreground mb-1">解析状态</p>
              <p className="text-xs font-medium text-foreground">{getStatusText(site.parsingStatus || "unknown", "parsing")}</p>
            </div>
            <div className="text-center p-2 bg-muted rounded-md">
              <p className="text-xs text-muted-foreground mb-1">生成状态</p>
              <p className="text-xs font-medium text-foreground">{getStatusText(site.modelGenerationStatus || "unknown", "model")}</p>
            </div>
          </div>
        </div>

        <div className="flex gap-2 pt-4 border-t">
          <Button
            variant="outline"
            size="sm"
            className="flex-1 bg-transparent"
            onClick={() => router.push(`/task-creation?siteId=${site.id}`)}
            title="为此站点创建任务"
          >
            <Plus className="w-4 h-4 mr-2" />
            创建任务
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={dbStatus === "running" ? handleStopDb : handleStartDb}
            title={dbStatus === "running" ? "停止数据库" : "启动数据库"}
          >
            <Play className="w-4 h-4" />
          </Button>
        </div>
      </CardContent>

      <SiteDetailModal site={site} open={showDetailModal} onOpenChange={setShowDetailModal} />
    </Card>
  )
}
