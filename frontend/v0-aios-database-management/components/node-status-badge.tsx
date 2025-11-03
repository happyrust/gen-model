"use client"

import { useEffect, useState } from "react"
import { Badge } from "@/components/ui/badge"
import { Card, CardContent } from "@/components/ui/card"
import { Server, Database, Activity, AlertCircle, CheckCircle, XCircle } from "lucide-react"
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"

interface NodeStatus {
  is_primary: boolean
  primary_url: string | null
  node_name: string | null
  litefs_status: any
  database_path: string
  timestamp: string
}

interface NodeStatusResponse {
  status?: string
  node?: NodeStatus
  message?: string
}

export function NodeStatusBadge() {
  const [status, setStatus] = useState<NodeStatus | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const fetchStatus = async () => {
    try {
      const response = await fetch("/api/node-status")
      if (!response.ok) throw new Error("Failed to fetch node status")
      const data: NodeStatusResponse = await response.json()

      if (!data?.node || data.status === "error") {
        throw new Error(data?.message || "节点状态数据为空")
      }

      setStatus(data.node)
      setError(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error")
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    fetchStatus()
    const interval = setInterval(fetchStatus, 10000)
    return () => clearInterval(interval)
  }, [])

  if (loading) {
    return (
      <Badge variant="outline" className="flex items-center gap-1">
        <Activity className="h-3 w-3 animate-pulse" />
        检测中...
      </Badge>
    )
  }

  if (error || !status) {
    return (
      <TooltipProvider>
        <Tooltip>
          <TooltipTrigger>
            <Badge variant="destructive" className="flex items-center gap-1">
              <XCircle className="h-3 w-3" />
              离线
            </Badge>
          </TooltipTrigger>
          <TooltipContent>
            <p>无法连接到节点状态 API</p>
            {error && <p className="text-xs text-muted-foreground">{error}</p>}
          </TooltipContent>
        </Tooltip>
      </TooltipProvider>
    )
  }

  const litefsAvailable = status.litefs_status && !status.litefs_status.error
  const isPrimary = status.is_primary
  const nodeName = status.node_name || "未命名节点"

  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger>
          <div className="flex items-center gap-2">
            {isPrimary ? (
              <Badge variant="default" className="flex items-center gap-1">
                <Server className="h-3 w-3" />
                主节点
              </Badge>
            ) : (
              <Badge variant="secondary" className="flex items-center gap-1">
                <Database className="h-3 w-3" />
                副本
              </Badge>
            )}

            {litefsAvailable ? (
              <Badge variant="outline" className="flex items-center gap-1 text-green-600 border-green-600">
                <CheckCircle className="h-3 w-3" />
                LiteFS
              </Badge>
            ) : (
              <Badge variant="outline" className="flex items-center gap-1 text-amber-600 border-amber-600">
                <AlertCircle className="h-3 w-3" />
                本地模式
              </Badge>
            )}
          </div>
        </TooltipTrigger>
        <TooltipContent className="w-80">
          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <span className="text-sm font-medium">节点信息</span>
              <span className="text-xs text-muted-foreground">{nodeName}</span>
            </div>
            <div className="space-y-1 text-xs">
              <div className="flex justify-between">
                <span className="text-muted-foreground">节点类型:</span>
                <span className="font-medium">{isPrimary ? "主节点 (可读写)" : "副本节点 (只读)"}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">数据库路径:</span>
                <span className="font-mono text-xs">{status.database_path}</span>
              </div>
              {!isPrimary && status.primary_url && (
                <div className="flex justify-between">
                  <span className="text-muted-foreground">主节点:</span>
                  <span className="font-mono text-xs">{status.primary_url}</span>
                </div>
              )}
              {litefsAvailable && (
                <div className="flex justify-between">
                  <span className="text-muted-foreground">LiteFS 状态:</span>
                  <span className="font-medium text-green-600">已连接</span>
                </div>
              )}
            </div>
          </div>
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}

export function NodeStatusCard() {
  const [status, setStatus] = useState<NodeStatus | null>(null)
  const [loading, setLoading] = useState(true)

  const fetchStatus = async () => {
    try {
      const response = await fetch("/api/node-status")
      if (!response.ok) throw new Error("Failed to fetch node status")
      const data: NodeStatusResponse = await response.json()
      if (!data?.node || data.status === "error") {
        throw new Error(data?.message || "节点状态数据为空")
      }
      setStatus(data.node)
    } catch (err) {
      console.error("Failed to fetch node status:", err)
      setStatus(null)
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    fetchStatus()
    const interval = setInterval(fetchStatus, 10000)
    return () => clearInterval(interval)
  }, [])

  if (loading) {
    return (
      <Card>
        <CardContent className="pt-6">
          <div className="flex items-center justify-center">
            <Activity className="h-8 w-8 animate-pulse text-muted-foreground" />
          </div>
        </CardContent>
      </Card>
    )
  }

  if (!status) {
    return (
      <Card className="border-destructive">
        <CardContent className="pt-6">
          <div className="flex items-center gap-2 text-destructive">
            <AlertCircle className="h-4 w-4" />
            <span>无法获取节点状态</span>
          </div>
        </CardContent>
      </Card>
    )
  }

  const litefsAvailable = status.litefs_status && !status.litefs_status.error

  return (
    <Card>
      <CardContent className="pt-6">
        <div className="space-y-4">
          <div className="flex items-center justify-between">
            <h3 className="text-lg font-semibold">节点状态</h3>
            <NodeStatusBadge />
          </div>

          <div className="space-y-2 text-sm">
            <div className="flex justify-between">
              <span className="text-muted-foreground">节点类型</span>
              <span className="font-medium">
                {status.is_primary ? "主节点 (Primary)" : "副本节点 (Replica)"}
              </span>
            </div>

            {status.node_name && (
              <div className="flex justify-between">
                <span className="text-muted-foreground">节点名称</span>
                <span className="font-medium">{status.node_name}</span>
              </div>
            )}

            <div className="flex justify-between">
              <span className="text-muted-foreground">数据库路径</span>
              <span className="font-mono text-xs">{status.database_path}</span>
            </div>

            {!status.is_primary && status.primary_url && (
              <div className="flex justify-between">
                <span className="text-muted-foreground">主节点地址</span>
                <a
                  href={status.primary_url}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="font-mono text-xs text-blue-600 hover:underline"
                >
                  {status.primary_url}
                </a>
              </div>
            )}

            <div className="flex justify-between">
              <span className="text-muted-foreground">LiteFS 状态</span>
              {litefsAvailable ? (
                <span className="flex items-center gap-1 text-green-600 font-medium">
                  <CheckCircle className="h-3 w-3" />
                  已连接
                </span>
              ) : (
                <span className="flex items-center gap-1 text-amber-600 font-medium">
                  <AlertCircle className="h-3 w-3" />
                  本地模式
                </span>
              )}
            </div>
          </div>

          {status.is_primary ? (
            <div className="rounded-lg bg-blue-50 p-3 text-sm text-blue-900 border border-blue-200">
              <p className="font-medium">✓ 当前节点为主节点</p>
              <p className="text-xs mt-1">可以执行所有读写操作</p>
            </div>
          ) : (
            <div className="rounded-lg bg-amber-50 p-3 text-sm text-amber-900 border border-amber-200">
              <p className="font-medium">⚠ 当前节点为副本</p>
              <p className="text-xs mt-1">只能执行读操作，写操作将自动重定向到主节点</p>
            </div>
          )}
        </div>
      </CardContent>
    </Card>
  )
}
