"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { useParams, useRouter } from "next/navigation"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  ArrowLeft,
  RefreshCw,
  Play,
  Trash2,
  Settings,
  AlertCircle,
  Clock,
  Users,
  Network,
  Loader2,
  Server,
  Globe,
  Wifi,
  MapPin,
  Database,
  History,
  Download,
} from "lucide-react"
import { Sidebar } from "@/components/sidebar"
import type {
  CollaborationGroup,
  SyncRecord,
  RemoteSite,
  RemoteSyncLogEntry,
  RemoteSyncDailyStat,
  RemoteSyncFlowStat,
  SiteMetadataEntry,
  SiteMetadataResponse,
} from "@/types/collaboration"
import { fetchSyncRecords, fetchSyncStatus, syncGroup } from "@/lib/api/collaboration"
import {
  getRemoteSyncEnv,
  envToGroup,
  activateRemoteSyncEnv,
  deleteRemoteSyncEnv,
  listRemoteSyncSites,
  siteToRemoteSite,
  fetchRemoteSyncLogs,
  fetchRemoteSyncDailyStats,
  fetchRemoteSyncFlowStats,
  fetchSiteMetadata,
  buildSiteMetadataDownloadUrl,
} from "@/lib/api/collaboration-adapter"
import { toast } from "sonner"
import { SyncFlowGraph } from "@/components/collaboration/sync-flow-graph"
import {
  ResponsiveContainer,
  AreaChart,
  Area,
  CartesianGrid,
  Tooltip as RechartsTooltip,
  XAxis,
  YAxis,
} from "recharts"

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
  const [remoteLogs, setRemoteLogs] = useState<RemoteSyncLogEntry[]>([])
  const [logsTotal, setLogsTotal] = useState(0)
  const [logsLoading, setLogsLoading] = useState(true)
  const [logsError, setLogsError] = useState<string | null>(null)
  const [dailyStats, setDailyStats] = useState<RemoteSyncDailyStat[]>([])
  const [flowStats, setFlowStats] = useState<RemoteSyncFlowStat[]>([])
  const [analyticsLoading, setAnalyticsLoading] = useState(true)
  const [analyticsError, setAnalyticsError] = useState<string | null>(null)
  const [metadataSiteId, setMetadataSiteId] = useState<string | null>(null)
  const [siteMetadata, setSiteMetadata] = useState<SiteMetadataResponse | null>(null)
  const [metadataLoading, setMetadataLoading] = useState(false)
  const [metadataError, setMetadataError] = useState<string | null>(null)
  const LOG_PAGE_SIZE = 20
  const [logStatusFilter, setLogStatusFilter] = useState("all")
  const [logDirectionFilter, setLogDirectionFilter] = useState("all")
  const [logSiteFilter, setLogSiteFilter] = useState("all")
  const [logPage, setLogPage] = useState(0)

  const fetchSitesForGroup = useCallback(
    async (options?: { showToast?: boolean }) => {
      if (!groupId) return
      setSiteLoading(true)
      setSiteError(null)
      try {
        const remoteSites = await listRemoteSyncSites(groupId)
        const mapped = remoteSites.map(siteToRemoteSite)
        setSites(mapped)
        setMetadataSiteId((current) => current ?? (mapped[0]?.id ?? null))
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

  const fetchRemoteSummary = useCallback(
    async (options?: { showToast?: boolean }) => {
      if (!groupId) return
      setAnalyticsLoading(true)
      setAnalyticsError(null)
      try {
        const [dailyResponse, flowResponse] = await Promise.all([
          fetchRemoteSyncDailyStats({ envId: groupId, days: 14 }),
          fetchRemoteSyncFlowStats({ envId: groupId, limit: 12 }),
        ])
        setDailyStats(dailyResponse.items ?? [])
        setFlowStats(flowResponse.items ?? [])
        if (options?.showToast) {
          toast.success("同步分析数据已刷新")
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : "加载同步统计数据失败"
        setAnalyticsError(message)
        setDailyStats([])
        setFlowStats([])
        if (options?.showToast) {
          toast.error(message)
        }
      } finally {
        setAnalyticsLoading(false)
      }
    },
    [groupId],
  )

  const fetchSiteMetadataFor = useCallback(
    async (siteId: string, options?: { refresh?: boolean; showToast?: boolean }) => {
      if (!siteId) {
        setSiteMetadata(null)
        return
      }
      setMetadataLoading(true)
      setMetadataError(null)
      try {
        const metadata = await fetchSiteMetadata(siteId, { refresh: options?.refresh })
        setSiteMetadata(metadata)
        if (options?.showToast) {
          toast.success("站点元数据已刷新")
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : "加载站点元数据失败"
        setMetadataError(message)
        setSiteMetadata(null)
        if (options?.showToast) {
          toast.error(message)
        }
      } finally {
        setMetadataLoading(false)
      }
    },
    [],
  )

  const fetchRemoteLogs = useCallback(
    async (options?: { showToast?: boolean }) => {
      if (!groupId) return
      setLogsLoading(true)
      setLogsError(null)
      try {
        const logsResponse = await fetchRemoteSyncLogs({
          envId: groupId,
          status: logStatusFilter === "all" ? undefined : logStatusFilter,
          direction: logDirectionFilter === "all" ? undefined : logDirectionFilter,
          targetSite: logSiteFilter === "all" ? undefined : logSiteFilter,
          limit: LOG_PAGE_SIZE,
          offset: logPage * LOG_PAGE_SIZE,
        })
        setRemoteLogs(logsResponse.items ?? [])
        setLogsTotal(logsResponse.total ?? 0)
        if (options?.showToast) {
          toast.success("同步日志已刷新")
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : "加载同步日志失败"
        setLogsError(message)
        setRemoteLogs([])
        if (options?.showToast) {
          toast.error(message)
        }
      } finally {
        setLogsLoading(false)
      }
    },
    [groupId, logStatusFilter, logDirectionFilter, logSiteFilter, logPage],
  )

  const handleRefreshAnalytics = useCallback(() => {
    void fetchRemoteSummary({ showToast: true })
    void fetchRemoteLogs({ showToast: true })
  }, [fetchRemoteSummary, fetchRemoteLogs])

  const handleRefreshMetadata = useCallback(() => {
    if (!metadataSiteId) return
    void fetchSiteMetadataFor(metadataSiteId, { refresh: true, showToast: true })
  }, [metadataSiteId, fetchSiteMetadataFor])

  const loadGroupData = useCallback(async () => {
    if (!groupId) return
    setLoading(true)
    setError(null)
    try {
      const env = await getRemoteSyncEnv(groupId)
      const groupData = envToGroup(env)
      setGroup(groupData)
      await Promise.all([
        fetchSitesForGroup(),
        fetchRecordsForGroup(),
        fetchStatus(),
      ])
      await fetchRemoteSummary()
    } catch (err) {
      const message = err instanceof Error ? err.message : "加载协同组失败"
      setError(message)
      toast.error(message)
      setSiteLoading(false)
      setSyncLoading(false)
      setAnalyticsLoading(false)
      setLogsLoading(false)
      setSites([])
      setSyncRecords([])
      setSyncStatus("未知")
      setSyncStatusError(null)
      setCurrentSync(null)
      setRemoteLogs([])
      setDailyStats([])
      setFlowStats([])
    } finally {
      setLoading(false)
    }
  }, [groupId, fetchSitesForGroup, fetchRecordsForGroup, fetchStatus, fetchRemoteSummary])

  useEffect(() => {
    if (groupId) {
      loadGroupData()
    }
  }, [groupId, loadGroupData])

  useEffect(() => {
    if (!groupId) return
    void fetchRemoteLogs()
  }, [groupId, fetchRemoteLogs])

  useEffect(() => {
    if (!metadataSiteId) {
      setSiteMetadata(null)
      return
    }
    void fetchSiteMetadataFor(metadataSiteId)
  }, [metadataSiteId, fetchSiteMetadataFor])

  useEffect(() => {
    if (sites.length === 0) {
      setMetadataSiteId(null)
      return
    }
    if (!metadataSiteId || !sites.some((site) => site.id === metadataSiteId)) {
      setMetadataSiteId(sites[0]?.id ?? null)
    }
  }, [sites, metadataSiteId])

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

  const handleMetadataDownload = useCallback(
    (entry: SiteMetadataEntry) => {
      if (!metadataSiteId) return
      if (!metadataBasePathRaw) {
        if (entry.download_url) {
          window.open(entry.download_url, "_blank")
        }
        return
      }

      let relative = entry.relative_path || ""
      if (!relative) {
        const normalizedBase = metadataBasePathRaw.replace(/\\/g, "/").replace(/\/+$/, "")
        const normalizedFile = (entry.file_path ?? "").replace(/\\/g, "/")
        if (normalizedFile.startsWith(normalizedBase)) {
          relative = normalizedFile.slice(normalizedBase.length).replace(/^\/+/, "")
        }
        if (!relative) {
          relative = entry.file_name
        }
      }

      const url = buildSiteMetadataDownloadUrl(metadataSiteId, relative)
      window.open(url, "_blank")
    },
    [metadataSiteId, metadataBasePathRaw],
  )

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

  const getLogStatusBadge = (status: string) => {
    const key = status?.toLowerCase()
    const mapping: Record<string, { label: string; variant: "default" | "secondary" | "destructive" | "outline" }> =
      {
        pending: { label: "待处理", variant: "outline" },
        running: { label: "进行中", variant: "secondary" },
        completed: { label: "完成", variant: "default" },
        failed: { label: "失败", variant: "destructive" },
        cancelled: { label: "已取消", variant: "outline" },
      }
    const record = mapping[key ?? ""] ?? { label: status || "未知", variant: "outline" }
    return <Badge variant={record.variant}>{record.label}</Badge>
  }

  const formatDateTime = (value?: string) => {
    if (!value) return "-"
    const date = new Date(value)
    if (Number.isNaN(date.getTime())) return value
    return date.toLocaleString()
  }

  const formatBytes = (value?: number) => {
    if (!value || value <= 0) return "0 B"
    const units = ["B", "KB", "MB", "GB", "TB"]
    const exponent = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1)
    const scaled = value / Math.pow(1024, exponent)
    return `${scaled.toFixed(scaled >= 10 ? 0 : 1)} ${units[exponent]}`
  }

  const formatNumber = (value: number) => {
    return new Intl.NumberFormat("zh-CN").format(value)
  }

  const getFlowSiteLabel = (flow: RemoteSyncFlowStat) => {
    const match =
      sites.find((site) => site.id === flow.target_site) ??
      sites.find((site) => site.name === flow.target_site)
    return match?.name ?? flow.target_site ?? "未知站点"
  }

  const getLogTargetLabel = (log: RemoteSyncLogEntry) => {
    const match =
      (log.site_id ? sites.find((site) => site.id === log.site_id) : undefined) ??
      (log.target_site ? sites.find((site) => site.id === log.target_site) : undefined) ??
      (log.target_site ? sites.find((site) => site.name === log.target_site) : undefined)
    return match?.name ?? log.target_site ?? log.site_id ?? "未知站点"
  }

  const currentTaskType = (() => {
    if (!currentSync) return undefined
    const raw = (currentSync as Record<string, unknown>)["task_type"]
    return typeof raw === "string" ? raw : undefined
  })()

  const logsSummary = useMemo(() => {
    return remoteLogs.reduce(
      (acc, log) => {
        const status = log.status?.toLowerCase()
        if (status === "completed") {
          acc.completed += 1
        } else if (status === "running" || status === "pending") {
          acc.running += 1
        } else if (status === "failed") {
          acc.failed += 1
        } else {
          acc.others += 1
        }
        return acc
      },
      { completed: 0, running: 0, failed: 0, others: 0 },
    )
  }, [remoteLogs])

  const metadataEntries = useMemo(() => {
    if (!siteMetadata?.metadata?.entries) return []
    return [...siteMetadata.metadata.entries].sort((a, b) => {
      const aTime = new Date(a.updated_at).getTime()
      const bTime = new Date(b.updated_at).getTime()
      if (Number.isNaN(aTime) || Number.isNaN(bTime)) {
        return Number.isNaN(aTime) ? 1 : -1
      }
      return bTime - aTime
    })
  }, [siteMetadata])

  const metadataSourceLabel = useMemo(() => {
    const source = siteMetadata?.source ?? "unknown"
    const mapping: Record<string, string> = {
      local_path: "本地目录",
      remote_http: "远程 HTTP",
      cache: "缓存",
      unknown: "未知",
    }
    return mapping[source] ?? source
  }, [siteMetadata])

  const metadataBasePathRaw = useMemo(() => {
    return siteMetadata?.local_base ?? siteMetadata?.cache_path ?? null
  }, [siteMetadata])

  const metadataBasePath = useMemo(() => {
    return metadataBasePathRaw ?? "未提供"
  }, [metadataBasePathRaw])

  const metadataRemoteHost = useMemo(() => siteMetadata?.http_base ?? siteMetadata?.metadata?.site_http_host ?? null, [siteMetadata])

  const dailyAggregates = useMemo(() => {
    return dailyStats.reduce(
      (acc, item) => {
        acc.total += item.total ?? 0
        acc.completed += item.completed ?? 0
        acc.failed += item.failed ?? 0
        acc.records += item.record_count ?? 0
        acc.bytes += item.total_bytes ?? 0
        return acc
      },
      { total: 0, completed: 0, failed: 0, records: 0, bytes: 0 },
    )
  }, [dailyStats])

  const flowAggregates = useMemo(() => {
    return flowStats.reduce(
      (acc, item) => {
        acc.total += item.total ?? 0
        acc.completed += item.completed ?? 0
        acc.failed += item.failed ?? 0
        acc.records += item.record_count ?? 0
        acc.bytes += item.total_bytes ?? 0
        return acc
      },
      { total: 0, completed: 0, failed: 0, records: 0, bytes: 0 },
    )
  }, [flowStats])

  const dailyChartData = useMemo(() => {
    if (!dailyStats.length) return []
    return [...dailyStats].reverse()
  }, [dailyStats])

  const totalLogPages = useMemo(() => {
    if (!logsTotal || logsTotal <= 0) {
      return 1
    }
    return Math.max(1, Math.ceil(logsTotal / LOG_PAGE_SIZE))
  }, [logsTotal])

  const handlePrevPage = useCallback(() => {
    setLogPage((page) => Math.max(page - 1, 0))
  }, [])

  const handleNextPage = useCallback(() => {
    setLogPage((page) => (page + 1 < totalLogPages ? page + 1 : page))
  }, [totalLogPages])

  useEffect(() => {
    const maxPageIndex = Math.max(0, totalLogPages - 1)
    if (logPage > maxPageIndex) {
      setLogPage(maxPageIndex)
    }
  }, [totalLogPages, logPage])

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
    <div className="min-h-screen bg-background">
      <Sidebar />
      <main className="ml-64 p-8">
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

        {/* Site Metadata */}
        <Card>
          <CardHeader className="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
            <div>
              <CardTitle>站点同步元数据</CardTitle>
              <CardDescription>从站点目录或远程 HTTP 读取同步文件、统计与下载链接</CardDescription>
            </div>
            <div className="flex flex-col gap-2 md:flex-row md:items-center">
              <Select
                value={metadataSiteId ?? ""}
                onValueChange={(value) => setMetadataSiteId(value)}
                disabled={siteLoading || sites.length === 0}
              >
                <SelectTrigger className="w-[220px]">
                  <SelectValue placeholder="选择站点" />
                </SelectTrigger>
                <SelectContent>
                  {sites.map((site) => (
                    <SelectItem key={site.id} value={site.id}>
                      {site.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Button
                variant="outline"
                size="sm"
                onClick={handleRefreshMetadata}
                disabled={!metadataSiteId || metadataLoading}
              >
                <RefreshCw className={`h-4 w-4 mr-2 ${metadataLoading ? "animate-spin" : ""}`} />
                刷新元数据
              </Button>
            </div>
          </CardHeader>
          <CardContent>
            {sites.length === 0 ? (
              <div className="flex flex-col items-center justify-center rounded-lg border border-dashed border-border bg-muted/30 py-10 text-center space-y-2">
                <Database className="h-8 w-8 text-muted-foreground" />
                <p className="text-sm text-muted-foreground">当前无站点，暂无法读取同步元数据信息</p>
              </div>
            ) : !metadataSiteId ? (
              <p className="text-sm text-muted-foreground">请选择需要查看的站点。</p>
            ) : metadataError ? (
              <Alert variant="destructive">
                <AlertCircle className="h-4 w-4" />
                <AlertTitle>加载元数据失败</AlertTitle>
                <AlertDescription>{metadataError}</AlertDescription>
              </Alert>
            ) : metadataLoading && !siteMetadata ? (
              <div className="h-48 w-full rounded-lg border border-dashed border-border/60 bg-muted/30 animate-pulse" />
            ) : siteMetadata && metadataEntries.length === 0 ? (
              <div className="flex flex-col items-center justify-center rounded-lg border border-dashed border-border bg-muted/20 py-10 text-center space-y-2">
                <History className="h-8 w-8 text-muted-foreground" />
                <p className="text-sm text-muted-foreground">尚未生成任何同步文件</p>
                <p className="text-xs text-muted-foreground">可稍后刷新或等待增量任务完成。</p>
              </div>
            ) : siteMetadata ? (
              <div className="space-y-4">
                <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
                  <div>
                    <p className="text-xs text-muted-foreground">数据来源</p>
                    <p className="text-sm font-medium">{metadataSourceLabel}</p>
                  </div>
                  <div>
                    <p className="text-xs text-muted-foreground">采集时间</p>
                    <p className="text-sm">{formatDateTime(siteMetadata.fetched_at)}</p>
                  </div>
                  <div>
                    <p className="text-xs text-muted-foreground">元数据生成</p>
                    <p className="text-sm">{formatDateTime(siteMetadata.metadata.generated_at)}</p>
                  </div>
                  <div>
                    <p className="text-xs text-muted-foreground">文件条目</p>
                    <p className="text-sm">
                      {formatNumber(siteMetadata.entry_count ?? metadataEntries.length)}
                    </p>
                  </div>
                </div>
                <div className="grid gap-4 md:grid-cols-2">
                  <div>
                    <p className="text-xs text-muted-foreground">本地目录 / 缓存</p>
                    <p className="text-xs break-all text-muted-foreground">{metadataBasePath}</p>
                  </div>
                  <div>
                    <p className="text-xs text-muted-foreground">远程访问</p>
                    <p className="text-xs break-all text-muted-foreground">
                      {metadataRemoteHost || "未配置"}
                    </p>
                  </div>
                </div>
                {siteMetadata.warnings && siteMetadata.warnings.length > 0 && (
                  <div className="space-y-2">
                    {siteMetadata.warnings.map((warning, index) => (
                      <Alert
                        key={index}
                        className="border border-amber-400/60 bg-amber-50 text-amber-900"
                      >
                        <AlertCircle className="h-4 w-4" />
                        <AlertTitle>注意</AlertTitle>
                        <AlertDescription>{warning}</AlertDescription>
                      </Alert>
                    ))}
                  </div>
                )}
                <ScrollArea className="h-80 rounded-lg border border-border/60">
                  <div className="divide-y divide-border/60">
                    {metadataEntries.map((entry) => (
                      <div
                        key={entry.file_name}
                        className="flex flex-col gap-3 p-4 md:flex-row md:items-center md:justify-between"
                      >
                        <div className="space-y-1">
                          <p className="text-sm font-medium">{entry.file_name}</p>
                          <p className="text-xs text-muted-foreground">
                            {formatBytes(entry.file_size)} · 记录{" "}
                            {formatNumber(entry.record_count ?? 0)} · 更新时间{" "}
                            {formatDateTime(entry.updated_at)}
                          </p>
                          <p className="text-xs text-muted-foreground break-all">
                            {entry.file_path}
                          </p>
                          {entry.file_hash && (
                            <p className="text-[11px] text-muted-foreground break-all">
                              哈希 {entry.file_hash}
                            </p>
                          )}
                        </div>
                        <div className="flex flex-wrap items-center gap-2">
                          {entry.direction && <Badge variant="outline">{entry.direction}</Badge>}
                          {entry.source_env && (
                            <Badge variant="secondary">来源 {entry.source_env}</Badge>
                          )}
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => handleMetadataDownload(entry)}
                          >
                            <Download className="h-4 w-4 mr-2" />
                            下载
                          </Button>
                        </div>
                      </div>
                    ))}
                  </div>
                </ScrollArea>
              </div>
            ) : (
              <div className="h-48 w-full rounded-lg border border-dashed border-border/60 bg-muted/30 flex items-center justify-center text-sm text-muted-foreground">
                暂无元数据可展示
              </div>
            )}
          </CardContent>
        </Card>

        {/* Remote Sync Analytics */}
        <Card>
            <CardHeader className="flex flex-row items-center justify-between">
              <div>
                <CardTitle>异地同步流向分析</CardTitle>
                <CardDescription>展示环境到各站点的同步流量与成功率</CardDescription>
              </div>
              <div className="flex items-center gap-2">
                {analyticsLoading && <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />}
                <Button variant="outline" size="sm" onClick={handleRefreshAnalytics} disabled={analyticsLoading}>
                  <RefreshCw className={`h-4 w-4 mr-2 ${analyticsLoading ? "animate-spin" : ""}`} />
                  刷新分析
                </Button>
              </div>
            </CardHeader>
            <CardContent>
              {analyticsError ? (
                <Alert variant="destructive">
                  <AlertCircle className="h-4 w-4" />
                  <AlertTitle>分析数据加载失败</AlertTitle>
                  <AlertDescription>{analyticsError}</AlertDescription>
                </Alert>
              ) : flowStats.length === 0 && !analyticsLoading ? (
                <div className="flex h-48 flex-col items-center justify-center text-sm text-muted-foreground">
                  暂无同步流向数据
                </div>
              ) : (
                <>
                  {analyticsLoading && flowStats.length === 0 ? (
                    <div className="h-80 w-full rounded-lg border border-dashed border-border/60 bg-muted/30 animate-pulse" />
                  ) : (
                    <SyncFlowGraph groupName={group.name} flows={flowStats} sites={sites} />
                  )}
                  <div className="mt-6 grid gap-4 md:grid-cols-4">
                    <div>
                      <p className="text-xs text-muted-foreground">总任务数</p>
                      <p className="text-xl font-semibold">{formatNumber(flowAggregates.total)}</p>
                    </div>
                    <div>
                      <p className="text-xs text-muted-foreground">成功任务</p>
                      <p className="text-xl font-semibold text-emerald-500">{formatNumber(flowAggregates.completed)}</p>
                    </div>
                    <div>
                      <p className="text-xs text-muted-foreground">失败任务</p>
                      <p className="text-xl font-semibold text-destructive">{formatNumber(flowAggregates.failed)}</p>
                    </div>
                    <div>
                      <p className="text-xs text-muted-foreground">累计流量</p>
                      <p className="text-xl font-semibold">{formatBytes(flowAggregates.bytes)}</p>
                    </div>
                  </div>
                  <div className="mt-6 space-y-3">
                    {flowStats.map((flow, index) => {
                      const successRate =
                        flow.total > 0 ? ((flow.completed ?? 0) / flow.total) * 100 : undefined
                      return (
                        <div
                          key={`${flow.target_site}-${flow.direction}-${index}`}
                          className="flex flex-col gap-2 rounded-lg border border-border p-3 md:flex-row md:items-center md:justify-between"
                        >
                          <div>
                            <p className="text-sm font-semibold">
                              {getFlowSiteLabel(flow)} · {flow.direction || "单向"}
                            </p>
                            <p className="text-xs text-muted-foreground">
                              成功 {formatNumber(flow.completed ?? 0)}/{formatNumber(flow.total ?? 0)} · 失败{" "}
                              {formatNumber(flow.failed ?? 0)} · 数据量 {formatBytes(flow.total_bytes)}
                            </p>
                          </div>
                          {successRate !== undefined && (
                            <Badge variant={successRate >= 90 ? "default" : successRate >= 60 ? "secondary" : "destructive"}>
                              成功率 {successRate.toFixed(1)}%
                            </Badge>
                          )}
                        </div>
                      )
                    })}
                  </div>
                </>
              )}
            </CardContent>
          </Card>

          <div className="grid gap-4 lg:grid-cols-2">
            <Card>
              <CardHeader className="flex flex-row items-center justify-between">
                <div>
                  <CardTitle>每日同步趋势</CardTitle>
                  <CardDescription>展示成功与失败任务的时间趋势</CardDescription>
                </div>
                {analyticsLoading && <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />}
              </CardHeader>
              <CardContent>
                {analyticsError ? (
                  <Alert variant="destructive">
                    <AlertCircle className="h-4 w-4" />
                    <AlertTitle>统计数据不可用</AlertTitle>
                    <AlertDescription>{analyticsError}</AlertDescription>
                  </Alert>
                ) : dailyChartData.length === 0 && !analyticsLoading ? (
                  <p className="py-6 text-center text-sm text-muted-foreground">暂无历史统计数据</p>
                ) : (
                  <>
                    <div className="h-72 w-full">
                      <ResponsiveContainer width="100%" height="100%">
                        <AreaChart data={dailyChartData}>
                          <CartesianGrid strokeDasharray="3 3" stroke="rgba(148, 163, 184, 0.35)" />
                          <XAxis dataKey="day" stroke="var(--muted-foreground)" tick={{ fontSize: 12 }} />
                          <YAxis stroke="var(--muted-foreground)" tick={{ fontSize: 12 }} />
                          <RechartsTooltip
                            contentStyle={{ borderRadius: 8, borderColor: "var(--border)", background: "var(--card)" }}
                          />
                          <Area
                            type="monotone"
                            dataKey="completed"
                            name="成功"
                            stroke="#22c55e"
                            fill="#22c55e33"
                            strokeWidth={2}
                          />
                          <Area type="monotone" dataKey="failed" name="失败" stroke="#ef4444" fill="#ef444422" strokeWidth={2} />
                        </AreaChart>
                      </ResponsiveContainer>
                    </div>
                    <div className="mt-5 grid gap-3 md:grid-cols-3">
                      <div>
                        <p className="text-xs text-muted-foreground">统计天数</p>
                        <p className="text-lg font-semibold">{dailyChartData.length}</p>
                      </div>
                      <div>
                        <p className="text-xs text-muted-foreground">累计成功</p>
                        <p className="text-lg font-semibold text-emerald-500">{formatNumber(dailyAggregates.completed)}</p>
                      </div>
                      <div>
                        <p className="text-xs text-muted-foreground">累计失败</p>
                        <p className="text-lg font-semibold text-destructive">{formatNumber(dailyAggregates.failed)}</p>
                      </div>
                      <div>
                        <p className="text-xs text-muted-foreground">累计记录数</p>
                        <p className="text-lg font-semibold">{formatNumber(dailyAggregates.records)}</p>
                      </div>
                      <div>
                        <p className="text-xs text-muted-foreground">累计流量</p>
                        <p className="text-lg font-semibold">{formatBytes(dailyAggregates.bytes)}</p>
                      </div>
                    </div>
                  </>
                )}
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="flex flex-row items-center justify-between">
                <div>
                  <CardTitle>实时同步日志</CardTitle>
                  <CardDescription>展示最近的 watcher / MQTT 任务执行记录</CardDescription>
                </div>
                {logsLoading && <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />}
              </CardHeader>
              <CardContent>
                {logsError ? (
                  <Alert variant="destructive">
                    <AlertCircle className="h-4 w-4" />
                    <AlertTitle>日志加载失败</AlertTitle>
                    <AlertDescription>{logsError}</AlertDescription>
                  </Alert>
                ) : remoteLogs.length === 0 && !logsLoading ? (
                  <p className="py-6 text-center text-sm text-muted-foreground">暂无最新同步日志</p>
                ) : (
                  <>
                    <div className="mb-4 flex flex-wrap items-center gap-3">
                      <Select
                        value={logStatusFilter}
                        onValueChange={(value) => {
                          setLogStatusFilter(value)
                          setLogPage(0)
                        }}
                      >
                        <SelectTrigger className="h-8 w-32 text-xs">
                          <SelectValue placeholder="状态筛选" />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="all">全部状态</SelectItem>
                          <SelectItem value="completed">成功</SelectItem>
                          <SelectItem value="running">进行中</SelectItem>
                          <SelectItem value="pending">待处理</SelectItem>
                          <SelectItem value="failed">失败</SelectItem>
                          <SelectItem value="cancelled">已取消</SelectItem>
                        </SelectContent>
                      </Select>
                      <Select
                        value={logDirectionFilter}
                        onValueChange={(value) => {
                          setLogDirectionFilter(value)
                          setLogPage(0)
                        }}
                      >
                        <SelectTrigger className="h-8 w-32 text-xs">
                          <SelectValue placeholder="方向筛选" />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="all">全部方向</SelectItem>
                          <SelectItem value="UPLOAD">上传</SelectItem>
                          <SelectItem value="DOWNLOAD">下载</SelectItem>
                        </SelectContent>
                      </Select>
                      <Select
                        value={logSiteFilter}
                        onValueChange={(value) => {
                          setLogSiteFilter(value)
                          setLogPage(0)
                        }}
                      >
                        <SelectTrigger className="h-8 w-40 text-xs">
                          <SelectValue placeholder="目标站点" />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="all">全部站点</SelectItem>
                          {sites.map((site) => (
                            <SelectItem key={site.id} value={site.id}>
                              {site.name || site.id}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                      <div className="ml-auto flex items-center gap-2 text-xs text-muted-foreground">
                        <Button variant="outline" size="sm" onClick={handlePrevPage} disabled={logPage === 0}>
                          上一页
                        </Button>
                        <span>
                          第 {Math.min(logPage + 1, totalLogPages)} / {totalLogPages} 页
                        </span>
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={handleNextPage}
                          disabled={logPage + 1 >= totalLogPages}
                        >
                          下一页
                        </Button>
                      </div>
                    </div>
                    <div className="mb-4 flex flex-wrap gap-3 text-xs text-muted-foreground">
                      <span>总记录：{formatNumber(logsTotal)}</span>
                      <span>成功：{formatNumber(logsSummary.completed)}</span>
                      <span>进行中：{formatNumber(logsSummary.running)}</span>
                      <span>失败：{formatNumber(logsSummary.failed)}</span>
                    </div>
                    <ScrollArea className="h-80 pr-2">
                      <div className="space-y-3">
                        {logsLoading && remoteLogs.length === 0
                          ? Array.from({ length: 5 }).map((_, index) => (
                              <div
                                key={`skeleton-${index}`}
                                className="animate-pulse rounded-lg border border-border/60 bg-muted/40 p-3 space-y-2"
                              >
                                <div className="h-4 w-36 rounded bg-muted-foreground/40" />
                                <div className="h-3 w-48 rounded bg-muted-foreground/30" />
                                <div className="h-3 w-60 rounded bg-muted-foreground/20" />
                              </div>
                            ))
                          : remoteLogs.map((log) => (
                              <div key={log.id} className="rounded-lg border border-border p-3">
                                <div className="flex flex-wrap items-center justify-between gap-2">
                                  <div className="flex items-center gap-2">
                                    {getLogStatusBadge(log.status)}
                                    <span className="text-xs text-muted-foreground">
                                      {formatDateTime(log.created_at)}
                                    </span>
                                  </div>
                                  <div className="text-xs text-muted-foreground">
                                    {log.direction || "未知方向"} · {getLogTargetLabel(log)}
                                  </div>
                                </div>
                                <p className="mt-2 truncate text-sm font-medium" title={log.file_path}>
                                  {log.file_path || "未记录文件路径"}
                                </p>
                                <div className="mt-2 flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
                                  <span>记录数 {formatNumber(log.record_count ?? 0)}</span>
                                  <span>大小 {formatBytes(log.file_size)}</span>
                                  {log.started_at && <span>开始 {formatDateTime(log.started_at)}</span>}
                                  {log.completed_at && <span>结束 {formatDateTime(log.completed_at)}</span>}
                                </div>
                                {log.error_message && (
                                  <p className="mt-2 text-xs text-destructive">{log.error_message}</p>
                                )}
                              </div>
                            ))}
                      </div>
                    </ScrollArea>
                  </>
                )}
              </CardContent>
            </Card>
          </div>

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
                {group ? <Badge variant="outline">{group!.status}</Badge> : null}
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
                          <div className="text-sm text-muted-foreground">
                            {site.api_url ?? "未配置 API 地址"}
                          </div>
                        </div>
                      </div>
                      <div className="flex items-center space-x-2">
                        {getSiteStatusBadge(site.status)}
                        <Badge variant="outline">{site.location ?? "未知环境"}</Badge>
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
