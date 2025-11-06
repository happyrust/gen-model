"use client"

import { useCallback, useEffect, useState } from "react"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Plus, RefreshCw, Network, AlertCircle } from "lucide-react"
import { Sidebar } from "@/components/sidebar"
import type { CollaborationGroup } from "@/types/collaboration"
import { listRemoteSyncEnvs, envToGroup } from "@/lib/api/collaboration-adapter"
import { CreateGroupDialog } from "@/components/collaboration/create-group-dialog"
import { toast } from "sonner"
import { getPublicApiBaseUrl } from "@/lib/env"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { GroupDetailDialog } from "@/components/collaboration/group-detail-dialog"

export default function CollaborationPage() {
  const [groups, setGroups] = useState<CollaborationGroup[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [createDialogOpen, setCreateDialogOpen] = useState(false)
  const [initialized, setInitialized] = useState(false)
  const apiBaseUrl = getPublicApiBaseUrl()
  const [activeGroup, setActiveGroup] = useState<CollaborationGroup | null>(null)
  const [detailOpen, setDetailOpen] = useState(false)

  const loadGroups = useCallback(async (options?: { showToast?: boolean }) => {
    setLoading(true)
    setError(null)
    try {
      const envs = await listRemoteSyncEnvs()
      // 确保 envs 是数组，处理可能的 API 响应格式差异
      const envsArray = Array.isArray(envs) ? envs : (envs && typeof envs === 'object' && 'items' in envs && Array.isArray(envs.items) ? envs.items : [])
      const mappedGroups = envsArray.map(envToGroup)
      setGroups(mappedGroups)
      if (options?.showToast) {
        toast.success("协同组数据已刷新")
      }
      return { success: true as const }
    } catch (err) {
      const message = err instanceof Error ? err.message : "加载协同组失败"
      setError(message)
      setGroups([])
      if (options?.showToast) {
        toast.error(message)
      }
      return { success: false as const, error: message }
    } finally {
      setLoading(false)
      setInitialized(true)
    }
  }, [])

  useEffect(() => {
    loadGroups()
  }, [loadGroups])

  const getStatusBadge = (status: string) => {
    const variants: Record<string, "default" | "secondary" | "destructive" | "outline"> = {
      Active: "default",
      Syncing: "secondary",
      Paused: "outline",
      Error: "destructive",
    }
    return (
      <Badge variant={variants[status] || "outline"}>
        {status === "Active" && "活跃"}
        {status === "Syncing" && "同步中"}
        {status === "Paused" && "已暂停"}
        {status === "Error" && "错误"}
      </Badge>
    )
  }

  const getTypeBadge = (type: string) => {
    const labels: Record<string, string> = {
      ConfigSharing: "配置共享",
      DataSync: "数据同步",
      TaskCoordination: "任务协调",
      Hybrid: "混合模式",
    }
    return <Badge variant="outline">{labels[type] || type}</Badge>
  }

  const handleCreateGroup = (group: CollaborationGroup) => {
    setGroups((prev) => [group, ...prev])
    setCreateDialogOpen(false)
    toast.success(`已创建协同组「${group.name}」`)
  }

  const handleViewDetail = (group: CollaborationGroup) => {
    setActiveGroup(group)
    setDetailOpen(true)
  }

  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      <main className="ml-64 p-8">
        <div className="max-w-7xl mx-auto space-y-6">
          {/* Header */}
          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-3xl font-bold tracking-tight">异地协同配置</h1>
              <p className="text-muted-foreground mt-2">管理多站点协同组，实现配置同步和数据协调</p>
            </div>
            <div className="flex items-center gap-3">
              <Button
                variant="outline"
                size="sm"
                onClick={() => loadGroups({ showToast: true })}
                disabled={loading}
              >
                <RefreshCw className={`h-4 w-4 mr-2 ${loading ? "animate-spin" : ""}`} />
                刷新
              </Button>
              <CreateGroupDialog
                open={createDialogOpen}
                onOpenChange={setCreateDialogOpen}
                onSuccess={handleCreateGroup}
              />
              <Button size="sm" onClick={() => setCreateDialogOpen(true)}>
                <Plus className="h-4 w-4 mr-2" />
                创建协同组
              </Button>
            </div>
          </div>

          {/* Config Warning */}
          {!apiBaseUrl && (
            <Alert variant="default" className="border-warning/60 bg-warning/10 text-warning-foreground">
              <AlertTitle>未配置后端网关地址</AlertTitle>
              <AlertDescription>
                协同功能需要配置 <code>NEXT_PUBLIC_API_BASE_URL</code>。请参考 <code>docs/REMOTE_COLLABORATION_DEV_PLAN.md</code>{" "}
                并在 <code>.env.local</code> 中填写正确的网关 URL。
              </AlertDescription>
            </Alert>
          )}

          {/* Stats */}
          <div className="grid gap-4 md:grid-cols-4">
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">协同组总数</CardTitle>
                <Network className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{groups.length}</div>
              </CardContent>
            </Card>
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">活跃组</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">
                  {groups.filter((g) => g.status === "Active").length}
                </div>
              </CardContent>
            </Card>
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">同步中</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">
                  {groups.filter((g) => g.status === "Syncing").length}
                </div>
              </CardContent>
            </Card>
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">错误</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold text-destructive">
                  {groups.filter((g) => g.status === "Error").length}
                </div>
              </CardContent>
            </Card>
          </div>

          {/* Error Message */}
          {error && (
            <Card className="border-destructive">
              <CardContent className="pt-6 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                <div className="flex items-center gap-2 text-destructive">
                  <AlertCircle className="h-4 w-4 flex-shrink-0" />
                  <span className="text-sm">{error}</span>
                </div>
                <div className="flex gap-2">
                  <Button variant="outline" size="sm" onClick={() => loadGroups({ showToast: true })}>
                    重试拉取
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => navigator.clipboard?.writeText(".env.local -> NEXT_PUBLIC_API_BASE_URL")}
                  >
                    复制配置提示
                  </Button>
                </div>
              </CardContent>
            </Card>
          )}

          {/* Groups List */}
          {!initialized && loading ? (
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
              {Array.from({ length: 3 }).map((_, index) => (
                <Card key={index} className="border-border/60 bg-muted/30 animate-pulse">
                  <CardHeader>
                    <div className="h-4 w-32 rounded bg-muted-foreground/30" />
                    <div className="mt-2 h-3 w-full rounded bg-muted-foreground/20" />
                  </CardHeader>
                  <CardContent className="space-y-3">
                    <div className="h-3 w-3/4 rounded bg-muted-foreground/20" />
                    <div className="h-3 w-2/3 rounded bg-muted-foreground/20" />
                    <div className="h-3 w-1/2 rounded bg-muted-foreground/20" />
                  </CardContent>
                </Card>
              ))}
            </div>
          ) : groups.length === 0 ? (
            <Card>
              <CardContent className="flex flex-col items-center justify-center py-12">
                <Network className="h-12 w-12 text-muted-foreground mb-4" />
                <p className="text-lg font-medium mb-2">还没有协同组</p>
                <p className="text-sm text-muted-foreground mb-4">创建第一个协同组来管理多站点配置</p>
                <Button onClick={() => setCreateDialogOpen(true)}>
                  <Plus className="h-4 w-4 mr-2" />
                  创建协同组
                </Button>
              </CardContent>
            </Card>
          ) : (
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
              {groups.map((group) => (
                <Card key={group.id} className="hover:shadow-md transition-shadow">
                  <CardHeader>
                    <div className="flex items-start justify-between">
                      <div className="space-y-1">
                        <CardTitle className="text-lg">{group.name}</CardTitle>
                        <CardDescription className="line-clamp-2">
                          {group.description || "暂无描述"}
                        </CardDescription>
                      </div>
                      {getStatusBadge(group.status)}
                    </div>
                  </CardHeader>
                  <CardContent>
                    <div className="space-y-3">
                      <div className="flex items-center justify-between text-sm">
                        <span className="text-muted-foreground">类型</span>
                        {getTypeBadge(group.group_type)}
                      </div>
                      <div className="flex items-center justify-between text-sm">
                        <span className="text-muted-foreground">站点数量</span>
                        <span className="font-medium">{group.site_ids?.length || 0}</span>
                      </div>
                      <div className="flex items-center justify-between text-sm">
                        <span className="text-muted-foreground">同步模式</span>
                        <span className="font-medium">
                          {group.sync_strategy?.mode === "OneWay" && "单向"}
                          {group.sync_strategy?.mode === "TwoWay" && "双向"}
                          {group.sync_strategy?.mode === "Manual" && "手动"}
                        </span>
                      </div>
                      <div className="flex items-center justify-between text-sm">
                        <span className="text-muted-foreground">位置</span>
                        <span className="font-medium">{group.location || "未指定"}</span>
                      </div>
                    </div>
                  </CardContent>
                  <CardFooter className="border-t border-border/60 pt-4">
                    <Button variant="outline" size="sm" onClick={() => handleViewDetail(group)}>
                      查看详情
                    </Button>
                  </CardFooter>
                </Card>
              ))}
              {loading && (
                <Card className="flex items-center justify-center gap-2 border-dashed border-border text-muted-foreground">
                  <CardContent className="flex items-center justify-center gap-2 py-10">
                    <RefreshCw className="h-4 w-4 animate-spin" />
                    <span className="text-sm">正在刷新协同组列表…</span>
                  </CardContent>
                </Card>
              )}
            </div>
          )}
        </div>
      </main>
      <GroupDetailDialog
        group={activeGroup}
        open={detailOpen}
        onOpenChange={(open) => {
          setDetailOpen(open)
          if (!open) {
            setActiveGroup(null)
          }
        }}
        onGroupUpdated={() => loadGroups()}
      />
    </div>
  )
}
