"use client"

import { useCallback, useEffect, useState } from "react"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Plus, RefreshCw, Network, AlertCircle } from "lucide-react"
import { Sidebar } from "@/components/sidebar"
import type { CollaborationGroup } from "@/types/collaboration"
import { listRemoteSyncEnvs, envToGroup } from "@/lib/api/collaboration-adapter"
import { CreateGroupDialog } from "@/components/collaboration/create-group-dialog"
import Link from "next/link"

export default function CollaborationPage() {
  const [groups, setGroups] = useState<CollaborationGroup[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [createDialogOpen, setCreateDialogOpen] = useState(false)

  const loadGroups = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const envs = await listRemoteSyncEnvs()
      const mappedGroups = envs.map(envToGroup)
      setGroups(mappedGroups)
    } catch (err) {
      setError(err instanceof Error ? err.message : "加载协同组失败")
      setGroups([])
    } finally {
      setLoading(false)
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
  }

  return (
    <div className="flex min-h-screen bg-background">
      <Sidebar />
      <main className="flex-1 p-8">
        <div className="max-w-7xl mx-auto space-y-6">
          {/* Header */}
          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-3xl font-bold tracking-tight">异地协同配置</h1>
              <p className="text-muted-foreground mt-2">管理多站点协同组，实现配置同步和数据协调</p>
            </div>
            <div className="flex items-center gap-3">
              <Button variant="outline" size="sm" onClick={loadGroups} disabled={loading}>
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
              <CardContent className="pt-6">
                <div className="flex items-center gap-2 text-destructive">
                  <AlertCircle className="h-4 w-4" />
                  <span>{error}</span>
                </div>
              </CardContent>
            </Card>
          )}

          {/* Groups List */}
          {loading && groups.length === 0 ? (
            <div className="flex items-center justify-center py-12">
              <RefreshCw className="h-8 w-8 animate-spin text-muted-foreground" />
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
                <Link key={group.id} href={`/collaboration/${group.id}`}>
                  <Card className="hover:shadow-md transition-shadow cursor-pointer">
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
                  </Card>
                </Link>
              ))}
            </div>
          )}
        </div>
      </main>
    </div>
  )
}