"use client"

import { useState } from "react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { ChevronLeft, ChevronRight, Plus, Trash2, Edit } from "lucide-react"
import type { Site } from "@/types/remote-sync"

interface StepSiteConfigProps {
  sites: Partial<Site>[]
  onChange: (sites: Partial<Site>[]) => void
  onNext: () => void
  onPrevious: () => void
}

export function StepSiteConfig({ sites, onChange, onNext, onPrevious }: StepSiteConfigProps) {
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editingIndex, setEditingIndex] = useState<number | null>(null)
  const [currentSite, setCurrentSite] = useState<Partial<Site>>({})
  const [errors, setErrors] = useState<Record<string, string>>({})

  const openAddDialog = () => {
    setCurrentSite({})
    setEditingIndex(null)
    setErrors({})
    setDialogOpen(true)
  }

  const openEditDialog = (index: number) => {
    setCurrentSite(sites[index])
    setEditingIndex(index)
    setErrors({})
    setDialogOpen(true)
  }

  const validate = () => {
    const newErrors: Record<string, string> = {}

    if (!currentSite.name || currentSite.name.trim().length === 0) {
      newErrors.name = "站点名称不能为空"
    }

    if (currentSite.httpHost && !/^https?:\/\/.+/.test(currentSite.httpHost)) {
      newErrors.httpHost = "HTTP 地址格式不正确（需要 http:// 或 https://）"
    }

    setErrors(newErrors)
    return Object.keys(newErrors).length === 0
  }

  const handleSave = () => {
    if (!validate()) return

    const newSites = [...sites]
    if (editingIndex !== null) {
      newSites[editingIndex] = currentSite
    } else {
      newSites.push(currentSite)
    }
    onChange(newSites)
    setDialogOpen(false)
  }

  const handleDelete = (index: number) => {
    const newSites = sites.filter((_, i) => i !== index)
    onChange(newSites)
  }

  return (
    <div className="space-y-6">
      {/* 站点列表 */}
      <div className="space-y-3">
        {sites.length === 0 ? (
          <div className="text-center py-8 border-2 border-dashed rounded-lg">
            <p className="text-muted-foreground mb-4">还没有添加站点</p>
            <Button onClick={openAddDialog}>
              <Plus className="h-4 w-4 mr-2" />
              添加站点
            </Button>
          </div>
        ) : (
          <>
            {sites.map((site, index) => (
              <div
                key={index}
                className="p-4 border rounded-lg flex items-center justify-between"
              >
                <div className="flex-1">
                  <h4 className="font-medium">{site.name}</h4>
                  <div className="text-sm text-muted-foreground space-y-1 mt-2">
                    {site.location && <div>位置: {site.location}</div>}
                    {site.httpHost && <div>HTTP: {site.httpHost}</div>}
                    {site.dbnums && <div>数据库: {site.dbnums}</div>}
                  </div>
                </div>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => openEditDialog(index)}
                  >
                    <Edit className="h-4 w-4" />
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => handleDelete(index)}
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>
              </div>
            ))}
            <Button variant="outline" onClick={openAddDialog} className="w-full">
              <Plus className="h-4 w-4 mr-2" />
              添加更多站点
            </Button>
          </>
        )}
      </div>

      {/* 导航按钮 */}
      <div className="flex items-center justify-between">
        <Button variant="outline" onClick={onPrevious}>
          <ChevronLeft className="h-4 w-4 mr-2" />
          上一步
        </Button>
        <Button onClick={onNext}>
          下一步
          <ChevronRight className="h-4 w-4 ml-2" />
        </Button>
      </div>

      {/* 添加/编辑站点对话框 */}
      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{editingIndex !== null ? "编辑站点" : "添加站点"}</DialogTitle>
            <DialogDescription>
              配置远程站点的基本信息和连接参数
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="siteName">
                站点名称 <span className="text-destructive">*</span>
              </Label>
              <Input
                id="siteName"
                placeholder="例如：上海站点-A"
                value={currentSite.name || ""}
                onChange={(e) => setCurrentSite({ ...currentSite, name: e.target.value })}
                className={errors.name ? "border-destructive" : ""}
              />
              {errors.name && <p className="text-sm text-destructive">{errors.name}</p>}
            </div>

            <div className="space-y-2">
              <Label htmlFor="siteLocation">位置</Label>
              <Input
                id="siteLocation"
                placeholder="例如：上海"
                value={currentSite.location || ""}
                onChange={(e) => setCurrentSite({ ...currentSite, location: e.target.value })}
              />
            </div>

            <div className="space-y-2">
              <Label htmlFor="httpHost">HTTP 地址</Label>
              <Input
                id="httpHost"
                placeholder="例如：http://shanghai-site-a.example.com:8080"
                value={currentSite.httpHost || ""}
                onChange={(e) => setCurrentSite({ ...currentSite, httpHost: e.target.value })}
                className={errors.httpHost ? "border-destructive" : ""}
              />
              {errors.httpHost && (
                <p className="text-sm text-destructive">{errors.httpHost}</p>
              )}
            </div>

            <div className="space-y-2">
              <Label htmlFor="dbnums">数据库编号</Label>
              <Input
                id="dbnums"
                placeholder="例如：8010,8011,8012（逗号分隔）"
                value={currentSite.dbnums || ""}
                onChange={(e) => setCurrentSite({ ...currentSite, dbnums: e.target.value })}
              />
            </div>

            <div className="space-y-2">
              <Label htmlFor="notes">备注</Label>
              <Textarea
                id="notes"
                placeholder="站点说明或备注信息"
                value={currentSite.notes || ""}
                onChange={(e) => setCurrentSite({ ...currentSite, notes: e.target.value })}
                rows={3}
              />
            </div>
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={() => setDialogOpen(false)}>
              取消
            </Button>
            <Button onClick={handleSave}>保存</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
