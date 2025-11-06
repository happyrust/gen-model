"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Badge } from "@/components/ui/badge"
import { Card, CardContent, CardFooter, CardHeader, CardTitle, CardDescription } from "@/components/ui/card"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { SiteSelector } from "@/components/collaboration/site-selector"
import { GroupGraph } from "@/components/collaboration/group-graph"
import { toast } from "sonner"
import {
  fetchCollaborationGroup,
  fetchGroupSites,
  removeSiteFromGroup,
  addSiteToGroup,
  updateCollaborationGroup,
  fetchSyncRecords,
  fetchSyncStatus,
  pauseGroupSync,
  syncGroup,
  createRemoteSite,
} from "@/lib/api/collaboration"
import type { CollaborationGroup, SyncRecord } from "@/types/collaboration"
import { getPublicApiBaseUrl, isCollaborationWsEnabled } from "@/lib/env"
import { AlertCircle, CalendarClock, Copy, Loader2, Pause, Play, Plus, RefreshCw, Trash2 } from "lucide-react"
import { cn } from "@/lib/utils"
import { useWebSocket } from "@/hooks/use-websocket"

function maskSensitive(value: string, tailVisible = 2) {
  if (!value) return "****"
  if (value.length <= tailVisible) {
    return "*".repeat(value.length)
  }
  const maskedLength = value.length - tailVisible
  return `${"*".repeat(maskedLength)}${value.slice(-tailVisible)}`
}

async function handleCopyValue(value: string, label: string) {
  try {
    await navigator.clipboard.writeText(value)
    toast.success(`${label} 已复制到剪贴板`)
  } catch (err) {
    console.error("复制失败:", err)
    toast.error(`复制${label}失败`)
  }
}

interface GroupDetailDialogProps {
  group: CollaborationGroup | null
  open: boolean
  onOpenChange: (open: boolean) => void
  onGroupUpdated?: () => void
}

interface GroupSite {
  id: string
  name: string
  location?: string
  status?: string
  isPrimary: boolean
  raw: Record<string, unknown>
}

interface ManageSitesDialogProps {
  group: CollaborationGroup
  sites: GroupSite[]
  open: boolean
  onOpenChange: (open: boolean) => void
  onSave: (selectedSiteIds: string[], primarySiteId: string) => Promise<void>
  saving: boolean
  selectorRefreshKey: number
  onSelectorRefresh: () => void
  remoteMetadata: Record<string, any>
  onRemoteMetadataChange: (next: Record<string, any>) => void
}

export function GroupDetailDialog({ group, open, onOpenChange, onGroupUpdated }: GroupDetailDialogProps) {
  const [detail, setDetail] = useState<CollaborationGroup | null>(null)
  const [sites, setSites] = useState<GroupSite[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [manageOpen, setManageOpen] = useState(false)
  const [savingSites, setSavingSites] = useState(false)
  const [syncRecords, setSyncRecords] = useState<SyncRecord[]>([])
  const [syncLoading, setSyncLoading] = useState(false)
  const [syncError, setSyncError] = useState<string | null>(null)
  const [syncStatus, setSyncStatus] = useState<string>("unknown")
  const apiBaseUrl = getPublicApiBaseUrl()
  const [remoteSiteMetadata, setRemoteSiteMetadata] = useState<Record<string, any>>({})
  const [selectorRefreshKey, setSelectorRefreshKey] = useState<number>(0)
  const [quickImportOpen, setQuickImportOpen] = useState(false)
  const [quickImportUrl, setQuickImportUrl] = useState("")
  const [quickImportLoading, setQuickImportLoading] = useState(false)
  const [quickImportError, setQuickImportError] = useState<string | null>(null)
  const collabWsEnabled = isCollaborationWsEnabled()
  const wsPath = useMemo(() => {
    if (!collabWsEnabled) return null
    const id = group?.id ?? detail?.id
    return id ? `/ws/collaboration/groups/${id}` : "/ws/collaboration/groups/idle"
  }, [collabWsEnabled, group?.id, detail?.id])
  const { lastMessage: wsMessage } = useWebSocket(wsPath)

  useEffect(() => {
    if (!collabWsEnabled || !wsMessage) return
    const message = wsMessage as any
    switch (message.type) {
      case "sync_status":
        setSyncStatus(message.status ?? "unknown")
        break
      case "sync_record":
        if (message.record) {
          setSyncRecords((prev) => [message.record as SyncRecord, ...prev].slice(0, 20))
        }
        break
      case "remote_site_metadata":
        if (message.site_id && message.metadata) {
          setRemoteSiteMetadata((prev) => ({
            ...prev,
            [message.site_id]: message.metadata,
          }))
        }
        if (message.refresh_sites) {
          setSelectorRefreshKey(Date.now())
        }
        break
      default:
        break
    }
  }, [collabWsEnabled, wsMessage])

  const activeGroupId = group?.id ?? null

  const loadDetail = useCallback(async () => {
    if (!activeGroupId || !apiBaseUrl) {
      return
    }
    setLoading(true)
    setError(null)
    try {
      const [groupInfo, groupSites] = await Promise.all([
        fetchCollaborationGroup(activeGroupId),
        fetchGroupSites(activeGroupId),
      ])
      setDetail(groupInfo)
      setSites(mapGroupSites(groupSites?.items ?? [], groupInfo))
      setRemoteSiteMetadata(
        (groupInfo.shared_config?.remote_sites as Record<string, any> | undefined) ?? {},
      )
    } catch (err) {
      const message = err instanceof Error ? err.message : "加载协同组详情失败"
      setError(message)
    } finally {
      setLoading(false)
    }
  }, [activeGroupId, apiBaseUrl])

  const loadSyncRecords = useCallback(async () => {
    if (!activeGroupId || !apiBaseUrl) {
      return
    }
    setSyncLoading(true)
    setSyncError(null)
    try {
      const response = await fetchSyncRecords(activeGroupId)
      const items = Array.isArray(response.items) ? response.items : []
      setSyncRecords(items)
    } catch (err) {
      const message = err instanceof Error ? err.message : "获取同步记录失败"
      setSyncError(message)
      setSyncRecords([])
    } finally {
      setSyncLoading(false)
    }
  }, [activeGroupId, apiBaseUrl])

  const loadSyncStatus = useCallback(async () => {
    if (!activeGroupId || !apiBaseUrl) {
      return
    }
    try {
      const status = await fetchSyncStatus(activeGroupId)
      setSyncStatus(status?.status ?? "unknown")
    } catch {
      setSyncStatus("unknown")
    }
  }, [activeGroupId, apiBaseUrl])

  useEffect(() => {
    if (open && activeGroupId) {
      if (!apiBaseUrl) {
        setError("未配置 NEXT_PUBLIC_API_BASE_URL，无法加载协同组详情。")
        return
      }
      void loadDetail()
      void loadSyncStatus()
      void loadSyncRecords()
      const interval = window.setInterval(() => {
        void loadSyncStatus()
        void loadSyncRecords()
      }, 10000)
      return () => {
        window.clearInterval(interval)
      }
    }
    setDetail(null)
    setSites([])
    setError(null)
    setSyncRecords([])
    setSyncStatus("unknown")
    setRemoteSiteMetadata({})
  }, [open, activeGroupId, apiBaseUrl, loadDetail, loadSyncStatus, loadSyncRecords])

  const handleRemoveSite = useCallback(
    async (siteId: string) => {
      if (!detail) return
      if (typeof window !== "undefined") {
        const siteName = getSiteName(sites, siteId)
        const confirmed = window.confirm(`确认将站点「${siteName}」移出协同组吗？`)
        if (!confirmed) return
      }
      try {
        await removeSiteFromGroup(detail.id, siteId)
        if (detail.primary_site_id === siteId) {
          await updateCollaborationGroup(detail.id, { primary_site_id: undefined })
        }
        toast.success("站点已移出协同组")
        await Promise.all([loadDetail(), loadSyncStatus(), loadSyncRecords()])
        onGroupUpdated?.()
      } catch (err) {
        toast.error(err instanceof Error ? err.message : "移除站点失败")
      }
    },
    [detail, loadDetail, loadSyncRecords, loadSyncStatus, onGroupUpdated, sites],
  )

  const handleSaveSites = useCallback(
    async (selectedSiteIds: string[], primarySiteId: string) => {
      if (!detail) return
      setSavingSites(true)
      try {
        if (selectedSiteIds.length === 0) {
          throw new Error("请至少选择一个需要同步的站点")
        }
        if (!primarySiteId) {
          throw new Error("请指定主站点后再保存")
        }

        const currentIds = new Set(sites.map((site) => site.id))
        const nextIds = new Set(selectedSiteIds)

        const toAdd = selectedSiteIds.filter((id) => !currentIds.has(id))
        const toRemove = Array.from(currentIds).filter((id) => !nextIds.has(id))

        for (const siteId of toAdd) {
          await addSiteToGroup(detail.id, siteId)
        }

        for (const siteId of toRemove) {
          await removeSiteFromGroup(detail.id, siteId)
        }

        const currentPrimary = detail.primary_site_id ?? sites.find((s) => s.isPrimary)?.id ?? ""
        if (primarySiteId !== currentPrimary) {
          await updateCollaborationGroup(detail.id, {
            primary_site_id: primarySiteId || undefined,
          })
        }

        const existingRemoteMeta =
          (detail.shared_config?.remote_sites as Record<string, any> | undefined) ?? {}
        const mergedRemoteMeta = {
          ...existingRemoteMeta,
          ...remoteSiteMetadata,
        }
        const filteredRemoteMeta = Object.fromEntries(
          Array.from(nextIds)
            .map((id) => [id, mergedRemoteMeta[id]])
            .filter(([, meta]) => meta),
        )

        const sharedConfig = {
          ...(detail.shared_config ?? {}),
          mqtt_primary_site_id: primarySiteId,
          mqtt_client_site_ids: selectedSiteIds.filter((id) => id !== primarySiteId),
          remote_sites: filteredRemoteMeta,
        }

        await updateCollaborationGroup(detail.id, {
          shared_config: sharedConfig,
        })

        toast.success("协同组站点已更新")
        setManageOpen(false)
        await Promise.all([loadDetail(), loadSyncStatus(), loadSyncRecords()])
        onGroupUpdated?.()
      } catch (err) {
        toast.error(err instanceof Error ? err.message : "更新站点失败")
      } finally {
        setSavingSites(false)
      }
    },
    [detail, sites, remoteSiteMetadata, loadDetail, loadSyncRecords, loadSyncStatus, onGroupUpdated],
  )

  const primarySiteId = useMemo(() => {
    return detail?.primary_site_id ?? sites.find((site) => site.isPrimary)?.id ?? ""
  }, [detail?.primary_site_id, sites])

  const selectedSiteIds = useMemo(() => sites.map((site) => site.id), [sites])

  const handleSyncStart = useCallback(
    async (groupId: string) => {
      try {
        setSyncLoading(true)
        await syncGroup(groupId, { force: true })
        toast.success("已触发同步任务")
        await Promise.all([loadSyncStatus(), loadSyncRecords()])
      } catch (err) {
        toast.error(err instanceof Error ? err.message : "触发同步失败")
      } finally {
        setSyncLoading(false)
      }
    },
    [loadSyncRecords, loadSyncStatus],
  )

  const handleSyncPause = useCallback(
    async (groupId: string) => {
      try {
        setSyncLoading(true)
        await pauseGroupSync(groupId)
        toast.success("同步已暂停")
        await Promise.all([loadSyncStatus(), loadSyncRecords()])
      } catch (err) {
        toast.error(err instanceof Error ? err.message : "暂停同步失败")
      } finally {
        setSyncLoading(false)
      }
    },
    [loadSyncRecords, loadSyncStatus],
  )
  const handleQuickImportSubmit = useCallback(async () => {
    if (!detail) {
      setQuickImportError("尚未加载协同组详情")
      return
    }
    const trimmedUrl = quickImportUrl.trim()
    if (!trimmedUrl) {
      setQuickImportError("请输入远程站点 URL")
      return
    }
    setQuickImportLoading(true)
    setQuickImportError(null)
    try {
      let resolvedUrl = trimmedUrl
      try {
        const urlObj = new URL(trimmedUrl)
        resolvedUrl = urlObj.toString()
      } catch {
        throw new Error("请输入合法的 URL，例如 https://example.com/metadata")
      }

      const response = await fetch(resolvedUrl, { cache: "no-store" })
      if (!response.ok) {
        throw new Error(`远程站点返回错误：HTTP ${response.status}`)
      }
      const metadata = await response.json()
      const defaultName =
        metadata.name ?? metadata.site_name ?? new URL(resolvedUrl).hostname ?? "远程站点"
      const apiUrl =
        metadata.api_url ?? metadata.base_url ?? metadata.endpoint ?? resolvedUrl

      if (!apiUrl) {
        throw new Error("远程站点响应中缺少 api_url 字段")
      }

      const created = await createRemoteSite({
        name: String(defaultName),
        api_url: String(apiUrl),
        auth_token: metadata.auth_token ? String(metadata.auth_token) : undefined,
        metadata,
      })

      const siteId = String(created.item?.id ?? apiUrl)
      await addSiteToGroup(detail.id, siteId)

      const primaryId =
        detail.primary_site_id ??
        sites.find((s) => s.isPrimary)?.id ??
        siteId
      const clientIds = Array.from(
        new Set(
          [...sites.filter((s) => !s.isPrimary).map((s) => s.id), siteId].filter(
            (id) => id && id !== primaryId,
          ),
        ),
      )

      const newRemoteMeta = {
        ...remoteSiteMetadata,
        [siteId]: {
          ...metadata,
          source_url: resolvedUrl,
          fetched_at: new Date().toISOString(),
        },
      }

      await updateCollaborationGroup(detail.id, {
        primary_site_id: primaryId || undefined,
        shared_config: {
          ...(detail.shared_config ?? {}),
          mqtt_primary_site_id: primaryId,
          mqtt_client_site_ids: clientIds,
          remote_sites: newRemoteMeta,
        },
      })

      toast.success(`已添加远程站点「${defaultName}」`)
      setRemoteSiteMetadata(newRemoteMeta)
      setSelectorRefreshKey(Date.now())
      setQuickImportOpen(false)
      setQuickImportUrl("")
      await Promise.all([loadDetail(), loadSyncStatus(), loadSyncRecords()])
    } catch (err) {
      setQuickImportError(err instanceof Error ? err.message : "导入远程站点失败")
    } finally {
      setQuickImportLoading(false)
    }
  }, [
    detail,
    quickImportUrl,
    remoteSiteMetadata,
    sites,
    loadDetail,
    loadSyncRecords,
    loadSyncStatus,
  ])

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="max-w-4xl">
        <DialogHeader>
          <DialogTitle>协同组详情</DialogTitle>
          <DialogDescription>
            {detail?.name ?? group?.name ?? "查看协同环境配置、站点成员与主站点设置"}
          </DialogDescription>
        </DialogHeader>

        {!apiBaseUrl && (
          <Alert variant="default" className="border-warning/60 bg-warning/10 text-warning-foreground">
            <AlertTitle>缺少网关配置</AlertTitle>
            <AlertDescription>
              协同组详情依赖 <code>NEXT_PUBLIC_API_BASE_URL</code>，请在 <code>.env.local</code> 中补齐后刷新页面。
            </AlertDescription>
          </Alert>
        )}

        {!collabWsEnabled && (
          <Alert variant="default" className="border-info/60 bg-info/10 text-info-foreground">
            <AlertTitle>实时同步未开启</AlertTitle>
            <AlertDescription>
              尚未启用 WebSocket 推送，将退回轮询模式。设置 <code>NEXT_PUBLIC_COLLAB_WS_ENABLED=true</code>{" "}
              并重启前端以实时接收同步更新。
            </AlertDescription>
          </Alert>
        )}

        {error && (
          <Alert variant="destructive">
            <AlertCircle className="h-4 w-4" />
            <AlertTitle>加载失败</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}

        {loading && (
          <div className="flex items-center justify-center py-12">
            <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
          </div>
        )}

        {!loading && !error && detail && (
          <div className="space-y-6">
            <Card>
              <CardHeader>
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <CardTitle className="text-xl">{detail.name}</CardTitle>
                    <CardDescription>{detail.description || "暂无描述"}</CardDescription>
                  </div>
                  <Badge variant={detail.status === "Active" ? "default" : "secondary"}>{statusText(detail.status)}</Badge>
                </div>
              </CardHeader>
              <CardContent className="grid grid-cols-1 gap-4 md:grid-cols-2">
                <InfoRow label="协同类型" value={groupTypeText(detail.group_type)} />
                <InfoRow label="主站点" value={primarySiteId ? getSiteName(sites, primarySiteId) : "未设置"} />
                <InfoRow label="同步模式" value={syncModeText(detail.sync_strategy?.mode)} />
                <InfoRow label="自动同步" value={detail.sync_strategy?.auto_sync ? "是" : "否"} />
                <InfoRow label="创建人" value={detail.creator || "未知"} />
                <InfoRow label="位置" value={detail.location || "未指定"} />
                <InfoRow label="同步状态" value={syncStatusText(syncStatus)} />
              </CardContent>
              <CardFooter className="flex flex-wrap gap-2 border-t pt-4">
                <Button variant="outline" size="sm" onClick={() => loadDetail()}>
                  <RefreshCw className="mr-2 h-4 w-4" />
                  刷新
                </Button>
                <Button variant="outline" size="sm" onClick={() => setManageOpen(true)} disabled={!apiBaseUrl}>
                  <Plus className="mr-2 h-4 w-4" />
                  管理站点
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => handleSyncStart(detail.id)}
                  disabled={syncLoading || !apiBaseUrl}
                >
                  <Play className="mr-2 h-4 w-4" />
                  立即同步
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => handleSyncPause(detail.id)}
                  disabled={syncLoading || !apiBaseUrl}
                >
                  <Pause className="mr-2 h-4 w-4" />
                  暂停同步
                </Button>
              </CardFooter>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>成员站点</CardTitle>
                <CardDescription>协同组中参与同步的本地与远程站点</CardDescription>
              </CardHeader>
              <CardContent className="space-y-3">
                {sites.length === 0 ? (
                  <div className="rounded-lg border border-dashed border-border p-6 text-center text-sm text-muted-foreground">
                    尚未添加站点，点击「管理站点」开始选择。
                  </div>
                ) : (
                  sites.map((site) => (
                    <div
                      key={site.id}
                      className="flex items-center justify-between rounded-lg border border-border/60 bg-muted/30 px-4 py-3"
                    >
                      <div className="flex flex-col">
                        <span className="font-medium text-sm text-foreground">{site.name}</span>
                        <span className="text-xs text-muted-foreground">
                          {site.location || "未知位置"}
                          {site.status ? ` · 状态 ${site.status}` : ""}
                        </span>
                      </div>
                      <div className="flex items-center gap-2">
                        {site.isPrimary && <Badge>主站点</Badge>}
                        <Button
                          variant="ghost"
                          size="icon"
                          className={cn("text-muted-foreground hover:text-destructive")}
                          onClick={() => handleRemoveSite(site.id)}
                        >
                          <Trash2 className="h-4 w-4" />
                          <span className="sr-only">移除站点</span>
                        </Button>
                      </div>
                    </div>
                  ))
                )}
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>MQTT 拓扑图</CardTitle>
                <CardDescription>主站点作为 MQTT 服务端，其他站点通过客户端方式连接。</CardDescription>
              </CardHeader>
              <CardContent>
                <GroupGraph
                  group={detail}
                  sites={sites.map((site) => ({
                    id: site.id,
                    name: site.name,
                    isPrimary: site.isPrimary || site.id === primarySiteId,
                    status: site.status,
                  }))}
                  onPrimaryNodeClick={() => {
                    if (!apiBaseUrl) {
                      toast.error("未配置 API 网关，无法导入远程站点")
                      return
                    }
                    setQuickImportUrl("")
                    setQuickImportError(null)
                    setQuickImportOpen(true)
                  }}
                />
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>同步记录</CardTitle>
                <CardDescription>查看最近一次同步的状态和结果</CardDescription>
              </CardHeader>
              <CardContent className="space-y-3">
                {syncLoading ? (
                  <div className="flex items-center gap-2 text-sm text-muted-foreground">
                    <Loader2 className="h-4 w-4 animate-spin" />
                    正在加载同步记录…
                  </div>
                ) : syncError ? (
                  <Alert variant="destructive">
                    <AlertCircle className="h-4 w-4" />
                    <AlertTitle>无法获取同步记录</AlertTitle>
                    <AlertDescription>{syncError}</AlertDescription>
                  </Alert>
                ) : syncRecords.length === 0 ? (
                  <div className="rounded-lg border border-dashed border-border p-6 text-center text-sm text-muted-foreground">
                    暂无同步记录，点击「立即同步」执行首个同步任务。
                  </div>
                ) : (
                  syncRecords.slice(0, 5).map((record) => (
                    <div
                      key={record.id ?? `${record.started_at}-${record.target_site_id ?? ""}`}
                      className="flex items-center justify-between rounded-lg border border-border/60 bg-muted/20 px-4 py-3"
                    >
                      <div className="flex items-center gap-3">
                        <Badge
                          variant={
                            record.status === "Success"
                              ? "default"
                              : record.status === "Failed"
                                ? "destructive"
                                : "secondary"
                          }
                        >
                          {syncStatusBadgeText(record.status)}
                        </Badge>
                        <div className="flex flex-col text-sm">
                          <span>{record.sync_type || "未知类型"}</span>
                          <span className="flex items-center gap-1 text-xs text-muted-foreground">
                            <CalendarClock className="h-3 w-3" />
                            {formatTimestamp(record.completed_at || record.started_at)}
                          </span>
                        </div>
                      </div>
                      <div className="max-w-xs text-right text-xs text-muted-foreground">
                        {record.error_message
                          ? `失败原因：${record.error_message}`
                          : formatSyncSummary(record)}
                      </div>
                    </div>
                  ))
                )}
              </CardContent>
              <CardFooter className="border-t pt-4">
                <Button variant="outline" size="sm" onClick={loadSyncRecords} disabled={syncLoading}>
                  <RefreshCw className={`mr-2 h-4 w-4 ${syncLoading ? "animate-spin" : ""}`} />
                  刷新记录
                </Button>
              </CardFooter>
            </Card>
          </div>
        )}

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            关闭
          </Button>
        </DialogFooter>

        {detail && (
          <ManageSitesDialog
            group={detail}
            sites={sites}
            open={manageOpen}
            onOpenChange={setManageOpen}
            onSave={handleSaveSites}
            saving={savingSites}
            selectorRefreshKey={selectorRefreshKey}
            onSelectorRefresh={() => setSelectorRefreshKey(Date.now())}
            remoteMetadata={remoteSiteMetadata}
            onRemoteMetadataChange={setRemoteSiteMetadata}
          />
        )}
      </DialogContent>
    </Dialog>
      <Dialog
        open={quickImportOpen}
        onOpenChange={(open) => {
          setQuickImportOpen(open)
          if (!open) {
            setQuickImportUrl("")
            setQuickImportError(null)
          }
        }}
      >
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>快速导入远程站点</DialogTitle>
            <DialogDescription>
              通过点击拓扑图主站点，可在此直接录入远程站点 URL 并加入当前协同组。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="quick-import-url" className="text-sm font-medium text-foreground">
                站点配置 URL
              </Label>
              <Input
                id="quick-import-url"
                value={quickImportUrl}
                onChange={(event) => setQuickImportUrl(event.target.value)}
                placeholder="https://remote-site.example.com/api/site-metadata"
                disabled={quickImportLoading}
              />
              {quickImportError && <p className="text-xs text-destructive">{quickImportError}</p>}
            </div>
            <p className="text-xs text-muted-foreground">
              URL 应返回包含 `api_url`、`mqtt.host` 等字段的 JSON。导入后系统会自动将站点加入当前协同组。
            </p>
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setQuickImportOpen(false)}
              disabled={quickImportLoading}
            >
              取消
            </Button>
            <Button onClick={() => void handleQuickImportSubmit()} disabled={quickImportLoading}>
              {quickImportLoading ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
              导入
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}

function ManageSitesDialog({
  group,
  sites,
  open,
  onOpenChange,
  onSave,
  saving,
  selectorRefreshKey,
  onSelectorRefresh,
  remoteMetadata,
  onRemoteMetadataChange,
}: ManageSitesDialogProps) {
  const [selectedSiteIds, setSelectedSiteIds] = useState<string[]>([])
  const [primarySiteId, setPrimarySiteId] = useState<string>("")
  const [importUrl, setImportUrl] = useState<string>("")
  const [importing, setImporting] = useState(false)
  const [importError, setImportError] = useState<string | null>(null)
  const [refreshingSiteId, setRefreshingSiteId] = useState<string | null>(null)

  useEffect(() => {
    if (open) {
      setSelectedSiteIds(sites.map((site) => site.id))
      const primaryId = group.primary_site_id ?? sites.find((site) => site.isPrimary)?.id ?? ""
      setPrimarySiteId(primaryId)
    }
  }, [open, group.primary_site_id, sites])

  const handleSave = async () => {
    await onSave(selectedSiteIds, primarySiteId)
  }

  const handleImportRemoteSite = async () => {
    const trimmedUrl = importUrl.trim()
    if (!trimmedUrl) {
      setImportError("请输入远程站点 URL")
      return
    }

    setImporting(true)
    setImportError(null)
    try {
      let resolvedUrl = trimmedUrl
      try {
        const urlObj = new URL(trimmedUrl)
        resolvedUrl = urlObj.toString()
      } catch {
        throw new Error("请输入合法的 URL，例如 https://example.com/metadata")
      }

      const response = await fetch(resolvedUrl, { cache: "no-store" })
      if (!response.ok) {
        throw new Error(`远程站点返回错误：HTTP ${response.status}`)
      }
      const metadata = await response.json()
      const defaultName =
        metadata.name ?? metadata.site_name ?? new URL(resolvedUrl).hostname ?? "远程站点"
      const apiUrl =
        metadata.api_url ?? metadata.base_url ?? metadata.endpoint ?? resolvedUrl

      if (!apiUrl) {
        throw new Error("远程站点响应中缺少 api_url 字段")
      }

      const created = await createRemoteSite({
        name: String(defaultName),
        api_url: String(apiUrl),
        auth_token: metadata.auth_token ? String(metadata.auth_token) : undefined,
        metadata,
      })

      const siteId = String(created.item?.id ?? apiUrl)
      setSelectedSiteIds((prev) => Array.from(new Set([...prev, siteId])))
      if (!primarySiteId) {
        setPrimarySiteId(siteId)
      }

      onRemoteMetadataChange({
        ...remoteMetadata,
        [siteId]: {
          ...metadata,
          source_url: resolvedUrl,
          fetched_at: new Date().toISOString(),
        },
      })

      onSelectorRefresh()
      toast.success(`已导入远程站点「${defaultName}」`)
      setImportUrl("")
    } catch (err) {
      setImportError(err instanceof Error ? err.message : "导入远程站点失败")
    } finally {
      setImporting(false)
    }
  }

  const handleRefreshRemoteSite = async (siteId: string) => {
    const meta = remoteMetadata[siteId]
    if (!meta?.source_url) {
      toast.error("缺少 source_url，无法自动刷新该站点。")
      return
    }
    setRefreshingSiteId(siteId)
    try {
      const response = await fetch(meta.source_url, { cache: "no-store" })
      if (!response.ok) {
        throw new Error(`远程站点返回错误：HTTP ${response.status}`)
      }
      const nextMeta = await response.json()
      onRemoteMetadataChange({
        ...remoteMetadata,
        [siteId]: {
          ...nextMeta,
          source_url: meta.source_url,
          refreshed_at: new Date().toISOString(),
        },
      })
      toast.success("远程站点元数据已刷新")
      onSelectorRefresh()
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "刷新远程站点失败")
    } finally {
      setRefreshingSiteId(null)
    }
  }

  const handleRemoveRemoteSite = (siteId: string) => {
    setSelectedSiteIds((prev) => prev.filter((id) => id !== siteId))
    if (primarySiteId === siteId) {
      setPrimarySiteId("")
    }
    const nextMeta = { ...remoteMetadata }
    delete nextMeta[siteId]
    onRemoteMetadataChange(nextMeta)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-3xl">
        <DialogHeader>
          <DialogTitle>管理协同组站点</DialogTitle>
          <DialogDescription>选择需要参与协同的站点，并设置主站点。</DialogDescription>
        </DialogHeader>

          <div className="space-y-4">
            <div className="space-y-2 rounded-lg border border-border/70 bg-muted/20 p-4">
              <Label className="text-sm font-medium text-foreground">通过 URL 导入远程站点</Label>
              <p className="text-xs text-muted-foreground">
                仅需提供远程站点的配置 URL，我们会自动获取其 MQTT、API 等配置。
              </p>
              <div className="flex flex-col gap-2 sm:flex-row">
                <Input
                  className="flex-1"
                  value={importUrl}
                  onChange={(event) => setImportUrl(event.target.value)}
                  placeholder="https://remote-site.example.com/api/site-metadata"
                />
                <Button onClick={handleImportRemoteSite} disabled={importing}>
                  {importing ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
                  导入站点
                </Button>
              </div>
              {importError && <p className="text-xs text-destructive">{importError}</p>}
            </div>

            {Object.keys(remoteMetadata).length > 0 && (
              <div className="space-y-2 rounded-lg border border-border/70 bg-muted/30 p-4 text-xs text-muted-foreground">
                <p className="mb-2 text-sm font-medium text-foreground">已导入的远程站点</p>
                <div className="space-y-2">
                  {Object.entries(remoteMetadata).map(([id, meta]) => (
                    <div
                      key={id}
                      className="flex flex-col gap-2 rounded-md border border-border bg-background/60 p-3 text-xs"
                    >
                      <div className="flex flex-wrap items-center justify-between gap-2">
                        <span className="font-medium text-foreground">
                          {meta?.name ?? meta?.site_name ?? "未命名站点"}（ID: {id}）
                        </span>
                        <div className="flex gap-2">
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={refreshingSiteId === id}
                            onClick={() => void handleRefreshRemoteSite(id)}
                          >
                            {refreshingSiteId === id ? (
                              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                            ) : null}
                            刷新
                          </Button>
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => handleRemoveRemoteSite(id)}
                          >
                            移除
                          </Button>
                        </div>
                      </div>
                      {meta?.source_url && <span>来源：{meta.source_url}</span>}
                      {meta?.mqtt?.host && (
                        <div className="flex flex-col gap-1">
                          <span>
                            MQTT：{meta.mqtt.host}:{meta.mqtt.port ?? "默认端口"}（用户：
                            {meta.mqtt.username ?? "未提供"}）
                          </span>
                          {meta.mqtt.password && (
                            <div className="flex flex-wrap items-center gap-2">
                              <span>MQTT 密码：</span>
                              <span className="font-mono text-foreground">
                                {maskSensitive(String(meta.mqtt.password))}
                              </span>
                              <Button
                                variant="ghost"
                                size="sm"
                                className="px-2"
                                onClick={() =>
                                  void handleCopyValue(String(meta.mqtt.password), "MQTT 密码")
                                }
                              >
                                <Copy className="h-3 w-3" />
                              </Button>
                            </div>
                          )}
                        </div>
                      )}
                      {meta?.auth_token && (
                        <div className="flex flex-wrap items-center gap-2">
                          <span>Auth Token：</span>
                          <span className="font-mono text-foreground">
                            {maskSensitive(String(meta.auth_token))}
                          </span>
                          <Button
                            variant="ghost"
                            size="sm"
                            className="px-2"
                            onClick={() =>
                              void handleCopyValue(String(meta.auth_token), "Auth Token")
                            }
                          >
                            <Copy className="h-3 w-3" />
                          </Button>
                        </div>
                      )}
                      {meta?.file_server?.url && (
                        <div className="flex flex-col gap-1">
                          <span>文件服务：{meta.file_server.url}</span>
                          <div className="flex flex-wrap items-center gap-2">
                            <span>用户名：</span>
                            <span className="font-mono text-foreground">
                              {meta.file_server.username ?? "未提供"}
                            </span>
                            {meta.file_server.username && (
                              <Button
                                variant="ghost"
                                size="sm"
                                className="px-2"
                                onClick={() =>
                                  void handleCopyValue(
                                    String(meta.file_server.username),
                                    "文件服务用户名",
                                  )
                                }
                              >
                                <Copy className="h-3 w-3" />
                              </Button>
                            )}
                          </div>
                          {meta.file_server.password && (
                            <div className="flex flex-wrap items-center gap-2">
                              <span>密码：</span>
                              <span className="font-mono text-foreground">
                                {maskSensitive(String(meta.file_server.password))}
                              </span>
                              <Button
                                variant="ghost"
                                size="sm"
                                className="px-2"
                                onClick={() =>
                                  void handleCopyValue(
                                    String(meta.file_server.password),
                                    "文件服务密码",
                                  )
                                }
                              >
                                <Copy className="h-3 w-3" />
                              </Button>
                            </div>
                          )}
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              </div>
            )}

            <SiteSelector
              selectedSites={selectedSiteIds}
              onSelectionChange={setSelectedSiteIds}
              primarySiteId={primarySiteId}
              onPrimaryChange={setPrimarySiteId}
              refreshKey={selectorRefreshKey}
            />
          <div className="border-t border-border/70" />
          <div className="text-xs text-muted-foreground space-y-1">
            <p>务必选择一个主站点：只有主站点需要配置 MQTT 服务器，其他站点会自动共享该配置。</p>
            <p>保存后将同步变更到后端，可能需要数秒完成。</p>
          </div>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button onClick={handleSave} disabled={saving || selectedSiteIds.length === 0 || !primarySiteId}>
            {saving ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
            保存
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function mapGroupSites(items: any[], group: CollaborationGroup): GroupSite[] {
  return items
    .map((item) => {
      const id = String(item?.site_id ?? item?.id ?? "")
      if (!id) return null
      return {
        id,
        name: String(item?.name ?? item?.site_name ?? "未命名站点"),
        location: typeof item?.location === "string" ? item.location : item?.site_location ? String(item.site_location) : undefined,
        status: typeof item?.status === "string" ? item.status : undefined,
        isPrimary: group.primary_site_id
          ? group.primary_site_id === id
          : Boolean(item?.is_primary),
        raw: item ?? {},
      } satisfies GroupSite
    })
    .filter(Boolean) as GroupSite[]
}

function statusText(status: string) {
  switch (status) {
    case "Active":
      return "活跃"
    case "Syncing":
      return "同步中"
    case "Paused":
      return "已暂停"
    case "Error":
      return "异常"
    case "Inactive":
      return "未启动"
    default:
      return status || "未知"
  }
}

function syncModeText(mode?: string) {
  switch (mode) {
    case "OneWay":
      return "单向同步"
    case "TwoWay":
      return "双向同步"
    case "Manual":
      return "手动触发"
    default:
      return "未知模式"
  }
}

function groupTypeText(type: string) {
  switch (type) {
    case "ConfigSharing":
      return "配置共享"
    case "DataSync":
      return "数据同步"
    case "TaskCoordination":
      return "任务协调"
    case "Hybrid":
      return "混合模式"
    default:
      return type
  }
}

function getSiteName(sites: GroupSite[], siteId: string) {
  return sites.find((site) => site.id === siteId)?.name ?? siteId
}

function InfoRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col">
      <span className="text-xs text-muted-foreground uppercase tracking-wide">{label}</span>
      <span className="text-sm font-medium text-foreground">{value}</span>
    </div>
  )
}

function syncStatusText(status: string) {
  switch (status) {
    case "running":
    case "Syncing":
      return "同步中"
    case "paused":
    case "Paused":
      return "已暂停"
    case "Success":
    case "completed":
      return "同步完成"
    case "Failed":
    case "failed":
      return "同步失败"
    case "Pending":
    case "pending":
      return "等待中"
    default:
      return status || "未知"
  }
}

function syncStatusBadgeText(status?: string) {
  return syncStatusText(status ?? "unknown")
}

function formatTimestamp(value?: string) {
  if (!value) return "未知时间"
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleString()
}

function formatSyncSummary(record: SyncRecord) {
  const parts: string[] = []
  if (record.source_site_id && record.target_site_id) {
    parts.push(`从 ${record.source_site_id} 到 ${record.target_site_id}`)
  }
  if (typeof record.data_size === "number") {
    parts.push(`数据量 ${formatDataSize(record.data_size)}`)
  }
  if (parts.length === 0) {
    return "同步完成"
  }
  return parts.join(" · ")
}

function formatDataSize(bytes: number) {
  if (!Number.isFinite(bytes) || bytes <= 0) return "未知"
  const units = ["B", "KB", "MB", "GB", "TB"]
  let value = bytes
  let unit = units.shift()!
  while (value >= 1024 && units.length > 0) {
    value /= 1024
    unit = units.shift()!
  }
  return `${value.toFixed(1)} ${unit}`
}
