"use client"

import { useCallback, useEffect, useState } from "react"
import { useParams, useRouter } from "next/navigation"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import {
  ArrowLeft,
  RefreshCw,
  Play,
  Trash2,
  Settings,
  AlertCircle,
  Clock,
  Network,
  Server,
  Globe,
  Wifi,
  MapPin,
} from "lucide-react"
import { Sidebar } from "@/components/sidebar"
import type { CollaborationGroup, SyncRecord, RemoteSite } from "@/types/collaboration"
import {
  getRemoteSyncEnv,
  envToGroup,
  activateRemoteSyncEnv,
  deleteRemoteSyncEnv,
  listRemoteSyncSites,
  siteToRemoteSite,
} from "@/lib/api/collaboration-adapter"
import { fetchSyncRecords, fetchSyncStatus, syncGroup } from "@/lib/api/collaboration"
import { toast } from "sonner"

export default function CollaborationDetailPage() {
  const params = useParams()
  const router = useRouter()
  const groupId = params.id as string

  const [group, setGroup] = useState<CollaborationGroup | null>(null)
  const [sites, setSites] = useState<RemoteSite[]>([])
  const [syncRecords, setSyncRecords] = useState<SyncRecord[]>([])
  const [syncStatus, setSyncStatus] = useState<string>("未知")
  const [syncStatusError, setSyncStatusError] = useState<string | null>(null)
  const [currentSync, setCurrentSync] = useState<Record<string, unknown> | null>(null)
  const [loading, setLoading] = useState(true)
  const [siteLoading, setSiteLoading] = useState(true)
  const [syncLoading, setSyncLoading] = useState(true)
  const [syncing, setSyncing] = useState(false)
  const [syncTriggering, setSyncTriggering] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [siteError, setSiteError] = useState<string | null>(null)
  const [syncRecordsError, setSyncRecordsError] = useState<string | null>(null)
  const [syncError, setSyncError] = useState<string | null>(null)

  const fetchSitesForGroup = useCallback(
    async (options?: { showToast?: boolean }) => {
      if (!groupId) return
      setSiteLoading(true)
      setSiteError(null)
      try {
        const remoteSites = await listRemoteSyncSites(groupId)
        const mapped = remoteSites.map(siteToRemoteSite)
        setSites(mapped)
        if (options?.showToast) {
          toast.success("站点信息已刷新")
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : "加载站点信息失败"
        setSiteError(message)
        setSites([])
        if (options?.showToast) {
          toast.error(message)
        }
      } finally {
        setSiteLoading(false)
      }
    },
    [groupId],
  )

  const fetchStatus = useCallback(
    async (options?: { showToast?: boolean }) => {
      if (!groupId) return
      setSyncStatusError(null)
      try {
        const result = await fetchSyncStatus(groupId)
        setSyncStatus(result.status)
        setCurrentSync(result.current_sync ?? null)
        if (options?.showToast) {
          toast.success("同步状态已更新")
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : "获取同步状态失败"
        setSyncStatusError(message)
        if (options?.showToast) {
          toast.error(message)
        }
      }
    },
    [groupId],
  )

  const fetchRecordsForGroup = useCallback(
    async (options?: { showToast?: boolean }) => {
      if (!groupId) return
      setSyncLoading(true)
      setSyncRecordsError(null)
      try {
        const { items } = await fetchSyncRecords(groupId)
        setSyncRecords(items ?? [])
        if (options?.showToast) {
          toast.success("同步记录已更新")
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : "加载同步记录失败"
        setSyncRecordsError(message)
        setSyncRecords([])
        if (options?.showToast) {
          toast.error(message)
        }
      } finally {
        setSyncLoading(false)
      }
    },
    [groupId],
  )

  const loadGroupData = useCallback(async () => {
    if (!groupId) return
    setLoading(true)
    setError(null)
    try {
      const env = await getRemoteSyncEnv(groupId)
      const groupData = envToGroup(env)
      setGroup(groupData)
      await Promise.all([fetchSitesForGroup(), fetchRecordsForGroup(), fetchStatus()])
    } catch (err) {
      const message = err instanceof Error ? err.message : "加载协同组失败"
      setError(message)
      toast.error(message)
      setSiteLoading(false)
      setSyncLoading(false)
      setSites([])
      setSyncRecords([])
      setSyncStatus("未知")
      setSyncStatusError(null)
      setCurrentSync(null)
    } finally {
      setLoading(false)
    }
  }, [groupId, fetchSitesForGroup, fetchRecordsForGroup, fetchStatus])

  useEffect(() => {
    if (groupId) {
      loadGroupData()
    }
  }, [groupId, loadGroupData])

  const handleSync = async () => {
    setSyncing(true)
    setError(null)
    try {
      await activateRemoteSyncEnv(groupId)
      toast.success("已激活协同环境，正在刷新数据")
      await loadGroupData()
    } catch (err) {
      const message = err instanceof Error ? err.message : "激活环境失败"
      setError(message)
      toast.error(message)
    } finally {
      setSyncing(false)
    }
  }

  const handleTriggerSync = async () => {
    setSyncTriggering(true)
    const toastId = toast.loading("正在触发同步任务...")
    try {
      await syncGroup(groupId, { force: true })
      toast.success("已触发同步任务", { id: toastId })
      await Promise.all([fetchRecordsForGroup(), fetchStatus()])
    } catch (err) {
      const message = err instanceof Error ? err.message : "触发同步失败"
      toast.error(message, { id: toastId })
    } finally {
      setSyncTriggering(false)
    }
  }

  const handleDelete = async () => {
    if (!confirm("确定要删除这个协同环境吗？此操作不可恢复。")) {
      return
    }
    try {
      await deleteRemoteSyncEnv(groupId)
      toast.success("协同环境已删除")
      router.push("/collaboration")
    } catch (err) {
      const message = err instanceof Error ? err.message : "删除失败"
      setError(message)
      toast.error(message)
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

  const getSiteStatusBadge = (status?: RemoteSite["status"]) => {
    const statusKey = status || "Offline"
    const variants: Record<string, { label: string; variant: "default" | "secondary" | "destructive" | "outline" }> = {
      Online: { label: "在线", variant: "default" },
      Connected: { label: "已连接", variant: "default" },
      Connecting: { label: "连接中", variant: "secondary" },
      Offline: { label: "离线", variant: "outline" },
      Disconnected: { label: "已断开", variant: "outline" },
      Failed: { label: "失败", variant: "destructive" },
    }
    const { label, variant } = variants[statusKey] || variants.Offline
    return <Badge variant={variant}>{label}</Badge>
  }

  const formatSyncStatus = (status: string) => {
    const labels: Record<string, string> = {
      InProgress: "同步进行中",
      Success: "同步完成",
      Failed: "同步失败",
      PartialSuccess: "部分成功",
    }
    return labels[status] || status || "未知"
  }

  const currentTaskType = (() => {
    if (!currentSync) return undefined
    const raw = (currentSync as Record<string, unknown>)["task_type"]
    return typeof raw === "string" ? raw : undefined
  })()

  if (loading && !group) {
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
              <Button variant="outline" size="sm" onClick={() => loadGroupData()}>
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
              <Button
                variant="outline"
                size="sm"
                onClick={handleTriggerSync}
                disabled={syncTriggering || syncing}
              >
                {syncTriggering ? (
                  <>
                    <RefreshCw className="h-4 w-4 mr-2 animate-spin" />
                    同步中...
                  </>
                ) : (
                  <>
                    <Network className="h-4 w-4 mr-2" />
                    触发同步
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
                <div className="text-2xl font-bold">{sites.length}</div>
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

          {/* Sites Information */}
          <Card>
            <CardHeader className="flex flex-row items-center justify-between">
              <div>
                <CardTitle>协同站点</CardTitle>
                <CardDescription>同步环境中包含的站点及连接状态</CardDescription>
              </div>
              <Button
                variant="outline"
                size="sm"
                onClick={() => fetchSitesForGroup({ showToast: true })}
                disabled={siteLoading || syncing || loading}
              >
                <RefreshCw className={`h-4 w-4 mr-2 ${siteLoading ? "animate-spin" : ""}`} />
                刷新站点
              </Button>
            </CardHeader>
            <CardContent>
              {siteLoading ? (
                <div className="grid gap-3 md:grid-cols-2">
                  {Array.from({ length: 4 }).map((_, index) => (
                    <div key={index} className="rounded-lg border border-border/60 bg-muted/30 p-4 animate-pulse space-y-3">
                      <div className="h-4 w-32 rounded bg-muted-foreground/30" />
                      <div className="h-3 w-20 rounded bg-muted-foreground/20" />
                      <div className="h-3 w-3/4 rounded bg-muted-foreground/20" />
                    </div>
                  ))}
                </div>
              ) : siteError ? (
                <div className="flex items-center gap-2 text-sm text-destructive">
                  <AlertCircle className="h-4 w-4" />
                  <span>{siteError}</span>
                </div>
              ) : sites.length === 0 ? (
                <div className="flex flex-col items-center justify-center rounded-lg border border-dashed border-border bg-muted/30 py-10 text-center space-y-2">
                  <Globe className="h-8 w-8 text-muted-foreground" />
                  <p className="text-sm text-muted-foreground">当前协同环境暂无已登记的站点</p>
                  <p className="text-xs text-muted-foreground">可在协同管理后台或通过 API 添加站点后刷新此页面</p>
                </div>
              ) : (
                <div className="grid gap-3 md:grid-cols-2">
                  {sites.map((site) => (
                    <Card key={site.id} className="border-border/60">
                      <CardContent className="p-4 space-y-3">
                        <div className="flex items-start justify-between gap-3">
                          <div>
                            <div className="flex items-center gap-2">
                              <Server className="h-4 w-4 text-muted-foreground" />
                              <p className="font-medium text-sm">{site.name}</p>
                            </div>
                            {site.location && (
                              <p className="text-xs text-muted-foreground flex items-center gap-1 mt-1">
                                <MapPin className="h-3 w-3" />
                                {site.location}
                              </p>
                            )}
                          </div>
                          {getSiteStatusBadge(site.status)}
                        </div>
                        <div className="grid grid-cols-2 gap-2 text-xs text-muted-foreground">
                          <div className="flex items-center gap-2">
                            <Wifi className="h-3 w-3" />
                            <span>{site.ip_address || "无 IP 信息"}</span>
                          </div>
                          <div className="flex items-center gap-2">
                            <Clock className="h-3 w-3" />
                            <span>
                              {site.last_sync
                                ? new Date(site.last_sync).toLocaleString()
                                : "未同步"}
                            </span>
                          </div>
                          {site.data_version && (
                            <div className="flex items-center gap-2">
                              <Network className="h-3 w-3" />
                              <span>版本 {site.data_version}</span>
                            </div>
                          )}
                          {typeof site.latency_ms === "number" && (
                            <div className="flex items-center gap-2">
                              <Globe className="h-3 w-3" />
                              <span>{site.latency_ms} ms</span>
                            </div>
                          )}
                        </div>
                      </CardContent>
                    </Card>
                  ))}
                </div>
              )}
            </CardContent>
          </Card>

          {/* Sync Records */}
          <Card>
            <CardHeader className="flex flex-row items-center justify-between">
              <div>
                <CardTitle>同步记录</CardTitle>
                <CardDescription>
                  {syncStatusError ? (
                    <span className="text-destructive">{syncStatusError}</span>
                  ) : (
                    <span>
                      当前状态：{formatSyncStatus(syncStatus)}
                      {currentTaskType ? ` · ${currentTaskType}` : ""}
                    </span>
                  )}
                </CardDescription>
              </div>
              <Button
                variant="outline"
                size="sm"
                onClick={() => fetchRecordsForGroup({ showToast: true })}
                disabled={syncLoading || syncing || loading}
              >
                <RefreshCw className={`h-4 w-4 mr-2 ${syncLoading ? "animate-spin" : ""}`} />
                刷新记录
              </Button>
            </CardHeader>
            <CardContent>
              {syncLoading ? (
                <div className="space-y-2">
                  {Array.from({ length: 4 }).map((_, index) => (
                    <div
                      key={index}
                      className="flex items-center justify-between rounded-lg border border-border/60 bg-muted/30 p-3 animate-pulse"
                    >
                      <div className="flex-1 space-y-2">
                        <div className="h-3 w-28 rounded bg-muted-foreground/30" />
                        <div className="h-3 w-40 rounded bg-muted-foreground/20" />
                      </div>
                      <div className="h-3 w-16 rounded bg-muted-foreground/20" />
                    </div>
                  ))}
                </div>
              ) : syncRecordsError ? (
                <div className="flex items-center gap-2 text-sm text-destructive">
                  <AlertCircle className="h-4 w-4" />
                  <span>{syncRecordsError}</span>
                </div>
              ) : syncRecords.length === 0 ? (
                <p className="text-sm text-muted-foreground text-center py-8">暂无同步记录</p>
              ) : (
                <div className="space-y-2">
                  {[...syncRecords]
                    .sort((a, b) => new Date(b.started_at).getTime() - new Date(a.started_at).getTime())
                    .slice(0, 10)
                    .map((record) => (
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

  useEffect(() => {
    if (!groupId) return
    const interval = setInterval(() => {
      fetchStatus()
    }, 15000)
    return () => clearInterval(interval)
  }, [groupId, fetchStatus])

  useEffect(() => {
    if (!groupId || syncStatus !== "InProgress") return
    const interval = setInterval(() => {
      fetchRecordsForGroup()
      fetchStatus()
    }, 10000)
    return () => clearInterval(interval)
  }, [groupId, syncStatus, fetchRecordsForGroup, fetchStatus])

  return (
    <div className="flex min-h-screen bg-background">
      <Sidebar />
      <main className="flex-1 p-8">
        <div className="space-y-6">
          {/* Header */}
          <div className="flex items-center justify-between">
            <div className="flex items-center space-x-4">
              <Button variant="outline" size="sm" onClick={() => router.back()}>
                <ArrowLeft className="h-4 w-4 mr-2" />
                返回
              </Button>
              <div className="flex items-center space-x-3">
                <Users className="h-6 w-6" />
                <h1 className="text-2xl font-bold">{group?.name || "协同组详情"}</h1>
                {group && <Badge variant="outline">{group.status}</Badge>}
              </div>
            </div>
            
            <div className="flex items-center space-x-2">
              <Button variant="outline" size="sm" onClick={handleSync} disabled={syncing}>
                {syncing ? <Loader2 className="h-4 w-4 mr-2 animate-spin" /> : <RefreshCw className="h-4 w-4 mr-2" />}
                {syncing ? "同步中..." : "手动同步"}
              </Button>
              <Button variant="outline" size="sm" onClick={handleTriggerSync} disabled={syncTriggering}>
                {syncTriggering ? <Loader2 className="h-4 w-4 mr-2 animate-spin" /> : <Play className="h-4 w-4 mr-2" />}
                {syncTriggering ? "触发中..." : "触发同步"}
              </Button>
              <Button variant="outline" size="sm" className="text-red-600" onClick={handleDelete}>
                <Trash2 className="h-4 w-4 mr-2" />
                删除组
              </Button>
            </div>
          </div>

          {/* Error Messages */}
          {error && (
            <Alert variant="destructive">
              <AlertCircle className="h-4 w-4" />
              <AlertTitle>加载失败</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}

          {siteError && (
            <Alert variant="destructive">
              <AlertCircle className="h-4 w-4" />
              <AlertTitle>站点加载失败</AlertTitle>
              <AlertDescription>{siteError}</AlertDescription>
            </Alert>
          )}

          {syncRecordsError && (
            <Alert variant="destructive">
              <AlertCircle className="h-4 w-4" />
              <AlertTitle>同步记录加载失败</AlertTitle>
              <AlertDescription>{syncRecordsError}</AlertDescription>
            </Alert>
          )}

          {syncError && (
            <Alert variant="destructive">
              <AlertCircle className="h-4 w-4" />
              <AlertTitle>同步状态获取失败</AlertTitle>
              <AlertDescription>{syncError}</AlertDescription>
            </Alert>
          )}

          {/* Status Cards */}
          <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
            <Card>
              <CardHeader className="pb-2">
                <CardTitle className="text-sm font-medium">同步状态</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="flex items-center space-x-2">
                  {getStatusBadge(syncStatus)}
                  <span className="text-sm text-muted-foreground">
                    {formatSyncStatus(syncStatus)}
                  </span>
                </div>
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="pb-2">
                <CardTitle className="text-sm font-medium">远程站点</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{sites.length}</div>
                <p className="text-xs text-muted-foreground">个站点</p>
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="pb-2">
                <CardTitle className="text-sm font-medium">同步记录</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{syncRecords.length}</div>
                <p className="text-xs text-muted-foreground">条记录</p>
              </CardContent>
            </Card>
          </div>

          {/* Remote Sites */}
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center space-x-2">
                <Globe className="h-5 w-5" />
                远程站点
                {siteLoading && <Loader2 className="h-4 w-4 animate-spin" />}
              </CardTitle>
            </CardHeader>
            <CardContent>
              {sites.length === 0 ? (
                <p className="text-muted-foreground text-center py-4">暂无远程站点</p>
              ) : (
                <div className="space-y-2">
                  {sites.map((site) => (
                    <div
                      key={site.id}
                      className="flex items-center justify-between p-3 border rounded-lg hover:bg-muted/50 transition"
                    >
                      <div className="flex items-center space-x-3">
                        <Database className="h-4 w-4 text-muted-foreground" />
                        <div>
                          <div className="font-medium">{site.name}</div>
                          <div className="text-sm text-muted-foreground">{site.url}</div>
                        </div>
                      </div>
                      <div className="flex items-center space-x-2">
                        {getSiteStatusBadge(site.status)}
                        <Badge variant="outline">{site.environment}</Badge>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </CardContent>
          </Card>

          {/* Sync Records */}
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center space-x-2">
                <History className="h-5 w-5" />
                同步记录
                {syncLoading && <Loader2 className="h-4 w-4 animate-spin" />}
              </CardTitle>
            </CardHeader>
            <CardContent>
              {syncRecords.length === 0 ? (
                <p className="text-muted-foreground text-center py-4">暂无同步记录</p>
              ) : (
                <div className="space-y-2">
                  {syncRecords.map((record) => (
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
