"use client"

import { useEffect, useState } from "react"
import { useParams, useRouter } from "next/navigation"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import {
  ArrowLeft,
  RefreshCw,
  Play,
  Pause,
  Trash2,
  Settings,
  AlertCircle,
  Clock,
  Network,
  Server,
} from "lucide-react"
import { Sidebar } from "@/components/sidebar"
import type { CollaborationGroup, SyncRecord } from "@/types/collaboration"
import { getRemoteSyncEnv, envToGroup, activateRemoteSyncEnv, deleteRemoteSyncEnv } from "@/lib/api/collaboration-adapter"

export default function CollaborationDetailPage() {
  const params = useParams()
  const router = useRouter()
  const groupId = params.id as string

  const [group, setGroup] = useState<CollaborationGroup | null>(null)
  const [syncRecords, setSyncRecords] = useState<SyncRecord[]>([])
  const [loading, setLoading] = useState(true)
  const [syncing, setSyncing] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const loadGroupData = async () => {
    setLoading(true)
    setError(null)
    try {
      const env = await getRemoteSyncEnv(groupId)
      const groupData = envToGroup(env)
      setGroup(groupData)
      setSyncRecords([])
    } catch (err) {
      setError(err instanceof Error ? err.message : "加载协同组失败")
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    if (groupId) {
      loadGroupData()
    }
  }, [groupId])

  const handleSync = async () => {
    setSyncing(true)
    setError(null)
    try {
      await activateRemoteSyncEnv(groupId)
      await loadGroupData()
    } catch (err) {
      setError(err instanceof Error ? err.message : "激活环境失败")
    } finally {
      setSyncing(false)
    }
  }

  const handleDelete = async () => {
    if (!confirm("确定要删除这个协同环境吗？此操作不可恢复。")) {
      return
    }
    try {
      await deleteRemoteSyncEnv(groupId)
      router.push("/collaboration")
    } catch (err) {
      setError(err instanceof Error ? err.message : "删除失败")
    }
  }

  const getStatusBadge = (status: string) => {
    const variants: Record<string, "default" | "secondary" | "destructive" | "outline"> = {
      Active: "default",
      Syncing: "secondary",
      Paused: "outline",
      Error: "destructive",
    }
    const labels: Record<string, string> = {
      Active: "活跃",
      Syncing: "同步中",
      Paused: "已暂停",
      Error: "错误",
    }
    return <Badge variant={variants[status] || "outline"}>{labels[status] || status}</Badge>
  }

  const getSyncStatusBadge = (status: string) => {
    const variants: Record<string, "default" | "secondary" | "destructive" | "outline"> = {
      InProgress: "secondary",
      Success: "default",
      Failed: "destructive",
      PartialSuccess: "outline",
    }
    const labels: Record<string, string> = {
      InProgress: "进行中",
      Success: "成功",
      Failed: "失败",
      PartialSuccess: "部分成功",
    }
    return <Badge variant={variants[status] || "outline"}>{labels[status] || status}</Badge>
  }

  if (loading) {
    return (
      <div className="flex min-h-screen bg-background">
        <Sidebar />
        <main className="flex-1 p-8">
          <div className="flex items-center justify-center h-full">
            <RefreshCw className="h-8 w-8 animate-spin text-muted-foreground" />
          </div>
        </main>
      </div>
    )
  }

  if (!group) {
    return (
      <div className="flex min-h-screen bg-background">
        <Sidebar />
        <main className="flex-1 p-8">
          <div className="max-w-7xl mx-auto">
            <Card className="border-destructive">
              <CardContent className="pt-6">
                <div className="flex items-center gap-2 text-destructive">
                  <AlertCircle className="h-4 w-4" />
                  <span>{error || "协同组不存在"}</span>
                </div>
              </CardContent>
            </Card>
            <Button onClick={() => router.push("/collaboration")} className="mt-4">
              <ArrowLeft className="h-4 w-4 mr-2" />
              返回列表
            </Button>
          </div>
        </main>
      </div>
    )
  }

  return (
    <div className="flex min-h-screen bg-background">
      <Sidebar />
      <main className="flex-1 p-8">
        <div className="max-w-7xl mx-auto space-y-6">
          {/* Header */}
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-4">
              <Button variant="ghost" size="sm" onClick={() => router.push("/collaboration")}>
                <ArrowLeft className="h-4 w-4" />
              </Button>
              <div>
                <div className="flex items-center gap-3">
                  <h1 className="text-3xl font-bold tracking-tight">{group.name}</h1>
                  {getStatusBadge(group.status)}
                </div>
                <p className="text-muted-foreground mt-1">{group.description || "暂无描述"}</p>
              </div>
            </div>
            <div className="flex items-center gap-2">
              <Button variant="outline" size="sm" onClick={loadGroupData}>
                <RefreshCw className="h-4 w-4 mr-2" />
                刷新
              </Button>
              <Button variant="outline" size="sm" onClick={handleSync} disabled={syncing}>
                {syncing ? (
                  <>
                    <RefreshCw className="h-4 w-4 mr-2 animate-spin" />
                    激活中...
                  </>
                ) : (
                  <>
                    <Play className="h-4 w-4 mr-2" />
                    激活环境
                  </>
                )}
              </Button>
              <Button variant="outline" size="sm">
                <Settings className="h-4 w-4 mr-2" />
                设置
              </Button>
              <Button variant="destructive" size="sm" onClick={handleDelete}>
                <Trash2 className="h-4 w-4 mr-2" />
                删除
              </Button>
            </div>
          </div>

          {/* Error Message */}
          {error && (
            <Card className="border-destructive">
              <CardContent className="pt-6">
                <div className="flex items-center gap-2 text-destructive">
                  <AlertCircle className="h-4 w-4" />
                  <span>{error}</span>
                </div>
              </CardContent>
            </Card>
          )}

          {/* Overview Cards */}
          <div className="grid gap-4 md:grid-cols-4">
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">站点数量</CardTitle>
                <Server className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{group.site_ids?.length || 0}</div>
              </CardContent>
            </Card>
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">同步模式</CardTitle>
                <Network className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">
                  {group.sync_strategy?.mode === "OneWay" && "单向"}
                  {group.sync_strategy?.mode === "TwoWay" && "双向"}
                  {group.sync_strategy?.mode === "Manual" && "手动"}
                </div>
              </CardContent>
            </Card>
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">同步频率</CardTitle>
                <Clock className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">
                  {group.sync_strategy?.interval_seconds >= 3600
                    ? `${Math.floor(group.sync_strategy.interval_seconds / 3600)}h`
                    : `${Math.floor(group.sync_strategy.interval_seconds / 60)}m`}
                </div>
              </CardContent>
            </Card>
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">同步记录</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{syncRecords.length}</div>
              </CardContent>
            </Card>
          </div>

          {/* Group Information */}
          <Card>
            <CardHeader>
              <CardTitle>协同组信息</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="grid gap-4 md:grid-cols-2">
                <div>
                  <p className="text-sm font-medium text-muted-foreground mb-1">协同组类型</p>
                  <p className="text-sm">
                    {group.group_type === "ConfigSharing" && "配置共享"}
                    {group.group_type === "DataSync" && "数据同步"}
                    {group.group_type === "TaskCoordination" && "任务协调"}
                    {group.group_type === "Hybrid" && "混合模式"}
                  </p>
                </div>
                <div>
                  <p className="text-sm font-medium text-muted-foreground mb-1">位置</p>
                  <p className="text-sm">{group.location || "未指定"}</p>
                </div>
                <div>
                  <p className="text-sm font-medium text-muted-foreground mb-1">自动同步</p>
                  <p className="text-sm">{group.sync_strategy?.auto_sync ? "已启用" : "已禁用"}</p>
                </div>
                <div>
                  <p className="text-sm font-medium text-muted-foreground mb-1">冲突解决</p>
                  <p className="text-sm">
                    {group.sync_strategy?.conflict_resolution === "PrimaryWins" && "主站点优先"}
                    {group.sync_strategy?.conflict_resolution === "LatestWins" && "最新更新优先"}
                    {group.sync_strategy?.conflict_resolution === "Manual" && "手动解决"}
                  </p>
                </div>
              </div>
            </CardContent>
          </Card>

          {/* Sync Records */}
          <Card>
            <CardHeader>
              <CardTitle>同步记录</CardTitle>
              <CardDescription>查看最近的同步操作历史</CardDescription>
            </CardHeader>
            <CardContent>
              {syncRecords.length === 0 ? (
                <p className="text-sm text-muted-foreground text-center py-8">暂无同步记录</p>
              ) : (
                <div className="space-y-2">
                  {syncRecords.slice(0, 10).map((record) => (
                    <div
                      key={record.id}
                      className="flex items-center justify-between p-3 border rounded-lg hover:bg-muted/50 transition"
                    >
                      <div className="flex-1">
                        <div className="flex items-center gap-2 mb-1">
                          {getSyncStatusBadge(record.status)}
                          <span className="text-sm font-medium">
                            {record.sync_type === "Config" && "配置同步"}
                            {record.sync_type === "FullData" && "全量数据同步"}
                            {record.sync_type === "IncrementalData" && "增量数据同步"}
                          </span>
                        </div>
                        <p className="text-xs text-muted-foreground">
                          {new Date(record.started_at).toLocaleString()}
                          {record.completed_at &&
                            ` - ${new Date(record.completed_at).toLocaleString()}`}
                        </p>
                        {record.error_message && (
                          <p className="text-xs text-destructive mt-1">{record.error_message}</p>
                        )}
                      </div>
                      {record.data_size && (
                        <div className="text-sm text-muted-foreground">
                          {(record.data_size / 1024 / 1024).toFixed(2)} MB
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              )}
            </CardContent>
          </Card>
        </div>
      </main>
    </div>
  )
}