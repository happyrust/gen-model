"use client"

import { useState } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Database, Users, Search, Plus } from "lucide-react"
import { Sidebar } from "@/components/sidebar"
import { FeatureCards } from "@/components/feature-cards"
import { SystemDashboard } from "@/components/system-dashboard"
import { EnhancedCreateSiteDialog } from "@/components/deployment-sites/enhanced-create-site-dialog"
import type { Site } from "@/components/deployment-sites/site-card"

const STATUS_DISPLAY: Record<Site["status"], { label: string; className: string }> = {
  running: { label: "启用中", className: "bg-success text-success-foreground" },
  deploying: { label: "部署中", className: "bg-primary text-primary-foreground" },
  configuring: { label: "配置中", className: "bg-muted text-muted-foreground" },
  failed: { label: "失败", className: "bg-destructive text-destructive-foreground" },
  paused: { label: "已暂停", className: "bg-muted text-muted-foreground" },
  stopped: { label: "已停止", className: "bg-muted text-muted-foreground" },
}

const ENVIRONMENT_LABELS: Record<Site["environment"], string> = {
  dev: "开发环境",
  test: "测试环境",
  staging: "预发布环境",
  prod: "生产环境",
}

export default function HomePage() {
  const [sites, setSites] = useState<Site[]>(() => [
    {
      id: "sample-site",
      name: "向导部署站点 - YCYK-E3D",
      status: "running",
      environment: "dev",
      owner: undefined,
      createdAt: "2025-09-16T09:12:35.391800+00:00",
      updatedAt: "2025-09-16T09:12:35.391800+00:00",
      url: undefined,
      description: undefined,
    },
  ])

  const handleSiteCreated = (site: Site) => {
    setSites((prev) => [site, ...prev])
  }

  return (
    <div className="min-h-screen bg-background">
      {/* Sidebar */}
      <Sidebar />

      {/* Main Content */}
      <div className="ml-64 p-8">
        {/* Header */}
        <div className="mb-8">
          <div className="flex items-center justify-between mb-4">
            <div>
              <h1 className="text-4xl font-bold text-foreground mb-2">AIOS 数据库管理平台</h1>
              <p className="text-lg text-muted-foreground">专业的数据库生成和空间树管理系统</p>
            </div>
            <Badge className="bg-success text-success-foreground px-4 py-2">
              系统运行正常 - 简单 Web UI 已成功启动
            </Badge>
          </div>
        </div>

        {/* System Dashboard */}
        <div className="mb-8">
          <SystemDashboard />
        </div>

        {/* Deploy Sites Section */}
        <Card className="mb-8 bg-card border-border">
          <CardHeader>
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-3">
                <Database className="h-6 w-6 text-primary" />
                <CardTitle className="text-xl text-card-foreground">部署站点</CardTitle>
              </div>
              <div className="flex items-center gap-3">
                <Button variant="outline" size="sm">
                  <Users className="h-4 w-4 mr-2" />
                  查看全部
                </Button>
                <Button variant="outline" size="sm">
                  刷新
                </Button>
                <EnhancedCreateSiteDialog
                  onCreateSite={handleSiteCreated}
                  trigger={
                    <Button size="sm" className="bg-success text-success-foreground hover:bg-success/90">
                      <Plus className="h-4 w-4 mr-2" />
                      创建站点
                    </Button>
                  }
                />
              </div>
            </div>
          </CardHeader>
          <CardContent>
            <div className="flex items-center gap-4 mb-6">
              <div className="relative flex-1">
                <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 h-4 w-4 text-muted-foreground" />
                <Input placeholder="搜索..." className="pl-10 bg-input border-border text-foreground" />
              </div>
              <Select defaultValue="all">
                <SelectTrigger className="w-32 bg-input border-border">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">全部</SelectItem>
                  <SelectItem value="running">运行中</SelectItem>
                  <SelectItem value="stopped">已停止</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-4">
              {sites.length === 0 ? (
                <Card className="border-dashed border-border bg-secondary/50">
                  <CardContent className="p-6 text-center text-muted-foreground">
                    暂无部署站点，点击“创建站点”开始新建。
                  </CardContent>
                </Card>
              ) : (
                sites.map((site) => {
                  const statusInfo = STATUS_DISPLAY[site.status]
                  const statusClass = statusInfo?.className ?? STATUS_DISPLAY.configuring.className
                  const statusLabel = statusInfo?.label ?? STATUS_DISPLAY.configuring.label

                  return (
                    <Card key={site.id} className="bg-secondary border-border">
                      <CardContent className="p-6">
                        <div className="flex items-center justify-between">
                          <div className="flex items-center gap-4">
                            <div className="w-12 h-12 bg-primary/10 rounded-lg flex items-center justify-center">
                              <Database className="h-6 w-6 text-primary" />
                            </div>
                            <div>
                              <h3 className="font-semibold text-secondary-foreground">{site.name}</h3>
                              <p className="text-sm text-muted-foreground">
                                {ENVIRONMENT_LABELS[site.environment]}
                                {site.owner ? ` · 负责人 ${site.owner}` : ""}
                              </p>
                              <p className="text-xs text-muted-foreground mt-1">{site.createdAt}</p>
                              {site.description && (
                                <p className="text-xs text-muted-foreground mt-2 line-clamp-2">{site.description}</p>
                              )}
                            </div>
                          </div>
                          <Badge className={statusClass}>{statusLabel}</Badge>
                        </div>
                      </CardContent>
                    </Card>
                  )
                })
              )}
            </div>
          </CardContent>
        </Card>

        {/* Enhanced Feature Cards */}
        <div className="mb-8">
          <div className="flex items-center justify-between mb-6">
            <div>
              <h2 className="text-2xl font-bold text-foreground">核心功能</h2>
              <p className="text-muted-foreground">管理和监控系统的主要功能模块</p>
            </div>
            <Button variant="outline" size="sm">
              查看全部功能
            </Button>
          </div>
          <FeatureCards />
        </div>
      </div>
    </div>
  )
}
