"use client"

import { useEffect, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Label } from "@/components/ui/label"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import { Loader2, Server, Globe } from "lucide-react"
import { fetchDeploymentSites } from "@/lib/api"
import { fetchRemoteSites } from "@/lib/api/collaboration"
import { getPublicApiBaseUrl } from "@/lib/env"

interface SiteSelectorProps {
  selectedSites: string[]
  onSelectionChange: (sites: string[]) => void
  primarySiteId?: string
  onPrimaryChange: (siteId: string) => void
  refreshKey?: number
}

export function SiteSelector({
  selectedSites,
  onSelectionChange,
  primarySiteId,
  onPrimaryChange,
  refreshKey,
}: SiteSelectorProps) {
  const [localSites, setLocalSites] = useState<any[]>([])
  const [remoteSites, setRemoteSites] = useState<any[]>([])
  const [loading, setLoading] = useState(true)
  const apiBaseUrl = getPublicApiBaseUrl()

  useEffect(() => {
    const loadSites = async () => {
      setLoading(true)
      try {
        if (!apiBaseUrl) {
          setLocalSites([])
          setRemoteSites([])
          return
        }
        const [localResult, remoteResult] = await Promise.all([
          fetchDeploymentSites({ page: 1, per_page: 100 }),
          fetchRemoteSites().catch(() => ({ items: [] })),
        ])
        setLocalSites(localResult.items || [])
        setRemoteSites(remoteResult.items || [])
      } catch (err) {
        console.error("Failed to load sites:", err)
      } finally {
        setLoading(false)
      }
    }
    loadSites()
  }, [apiBaseUrl, refreshKey])

  const toggleSite = (siteId: string) => {
    if (selectedSites.includes(siteId)) {
      onSelectionChange(selectedSites.filter((id) => id !== siteId))
      if (primarySiteId === siteId) {
        onPrimaryChange("")
      }
    } else {
      onSelectionChange([...selectedSites, siteId])
      if (!primarySiteId) {
        onPrimaryChange(siteId)
      }
    }
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center py-8">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    )
  }

  if (!apiBaseUrl) {
    return (
      <div className="space-y-2 rounded-lg border border-dashed border-warning/60 bg-warning/10 p-4 text-sm text-warning-foreground">
        <p>检测到未配置 <code>NEXT_PUBLIC_API_BASE_URL</code>，无法加载本地或远程站点列表。</p>
        <p>请在 <code>.env.local</code> 中填写网关地址后重新打开此对话框。</p>
      </div>
    )
  }

  return (
    <div className="space-y-4">
      <div>
        <Label className="text-base font-medium">选择站点</Label>
        <p className="text-sm text-muted-foreground mt-1">至少选择一个站点加入协同组</p>
      </div>

      {/* 本地站点 */}
      <div className="space-y-2">
        <div className="flex items-center gap-2">
          <Server className="h-4 w-4 text-muted-foreground" />
          <Label className="text-sm font-medium">本地站点</Label>
        </div>
        <div className="grid gap-2 max-h-64 overflow-y-auto">
          {localSites.length === 0 ? (
            <p className="text-sm text-muted-foreground py-4 text-center">没有可用的本地站点</p>
          ) : (
            localSites.map((site) => {
              const siteId = site.id as string
              const isSelected = selectedSites.includes(siteId)
              return (
                <Card
                  key={siteId}
                  className={`cursor-pointer transition-all ${
                    isSelected ? "border-primary bg-primary/5" : "hover:border-primary/50"
                  }`}
                  onClick={() => toggleSite(siteId)}
                >
                  <CardContent className="p-3">
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-3">
                        <input
                          type="checkbox"
                          checked={isSelected}
                          onChange={() => toggleSite(siteId)}
                          className="h-4 w-4 rounded border-gray-300"
                          onClick={(e) => e.stopPropagation()}
                        />
                        <div>
                          <p className="font-medium text-sm">{site.name}</p>
                          <p className="text-xs text-muted-foreground">
                            {site.env || "未知环境"} • {site.owner || "无负责人"}
                          </p>
                        </div>
                      </div>
                      {primarySiteId === siteId && (
                        <Badge variant="default" className="text-xs">
                          主站点
                        </Badge>
                      )}
                    </div>
                  </CardContent>
                </Card>
              )
            })
          )}
        </div>
      </div>

      {/* 远程站点 */}
      {remoteSites.length > 0 && (
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <Globe className="h-4 w-4 text-muted-foreground" />
            <Label className="text-sm font-medium">远程站点</Label>
          </div>
          <div className="grid gap-2 max-h-64 overflow-y-auto">
            {remoteSites.map((site) => {
              const siteId = site.id
              const isSelected = selectedSites.includes(siteId)
              return (
                <Card
                  key={siteId}
                  className={`cursor-pointer transition-all ${
                    isSelected ? "border-primary bg-primary/5" : "hover:border-primary/50"
                  }`}
                  onClick={() => toggleSite(siteId)}
                >
                  <CardContent className="p-3">
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-3">
                        <input
                          type="checkbox"
                          checked={isSelected}
                          onChange={() => toggleSite(siteId)}
                          className="h-4 w-4 rounded border-gray-300"
                          onClick={(e) => e.stopPropagation()}
                        />
                        <div>
                          <p className="font-medium text-sm">{site.name}</p>
                          <p className="text-xs text-muted-foreground">{site.api_url}</p>
                        </div>
                      </div>
                      {primarySiteId === siteId && (
                        <Badge variant="default" className="text-xs">
                          主站点
                        </Badge>
                      )}
                    </div>
                  </CardContent>
                </Card>
              )
            })}
          </div>
        </div>
      )}

      {/* 主站点选择 */}
      {selectedSites.length > 0 && (
        <div className="space-y-2 pt-4 border-t">
          <Label className="text-sm font-medium">设置主站点（可选）</Label>
          <p className="text-xs text-muted-foreground">主站点的配置将作为同步的源</p>
          <RadioGroup value={primarySiteId} onValueChange={onPrimaryChange}>
            {selectedSites.map((siteId) => {
              const site =
                localSites.find((s) => s.id === siteId) || remoteSites.find((s) => s.id === siteId)
              if (!site) return null
              return (
                <div key={siteId} className="flex items-center space-x-2">
                  <RadioGroupItem value={siteId} id={`primary-${siteId}`} />
                  <Label htmlFor={`primary-${siteId}`} className="font-normal cursor-pointer">
                    {site.name}
                  </Label>
                </div>
              )
            })}
          </RadioGroup>
        </div>
      )}
    </div>
  )
}
