"use client"

import { useEffect, useMemo, useState } from "react"

import { useDeploymentSiteDetail } from "@/hooks/use-deployment-site-detail"
import {
  createDeploymentSiteTask,
  exportDeploymentSiteConfig,
  healthcheckDeploymentSite,
} from "@/lib/api"
import type { TaskType } from "@/types/api"
import { cn } from "@/lib/utils"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"

interface DeploymentSiteDrawerProps {
  siteId: string | null
  open: boolean
  onOpenChange: (open: boolean) => void
}

const TASK_OPTIONS: Array<{ value: TaskType; label: string }> = [
  { value: "FullGeneration", label: "完整生成" },
  { value: "DataGeneration", label: "数据生成" },
  { value: "SpatialTreeGeneration", label: "空间树生成" },
  { value: "ParsePdmsData", label: "解析 PDMS" },
]

const PRIORITY_OPTIONS = [
  { value: "Urgent", label: "紧急" },
  { value: "High", label: "高" },
  { value: "Normal", label: "默认" },
  { value: "Low", label: "低" },
]

export function DeploymentSiteDrawer({ siteId, open, onOpenChange }: DeploymentSiteDrawerProps) {
  const { data, loading, error, refresh } = useDeploymentSiteDetail(siteId)
  const [taskType, setTaskType] = useState<TaskType>("FullGeneration")
  const [priority, setPriority] = useState("Normal")
  const [taskName, setTaskName] = useState("")
  const [creatingTask, setCreatingTask] = useState(false)
  const [checkingHealth, setCheckingHealth] = useState(false)
  const [exporting, setExporting] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [messageTone, setMessageTone] = useState<"success" | "error" | "info">("info")

  useEffect(() => {
    if (!open) return
    const handler = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onOpenChange(false)
      }
    }
    window.addEventListener("keydown", handler)
    return () => window.removeEventListener("keydown", handler)
  }, [open, onOpenChange])

  useEffect(() => {
    if (!open) return
    setMessage(null)
    setMessageTone("info")
    setTaskType("FullGeneration")
    setPriority("Normal")
    setTaskName("")
  }, [open, siteId])

  const infoEntries = useMemo(() => {
    if (!data) return []
    const entries: Array<[string, string]> = []
    entries.push(["状态", data.status ?? "--"])
    entries.push(["环境", data.env ?? "--"])
    entries.push(["负责人", data.owner ?? "--"])
    entries.push(["项目代码", data.project_code ? String(data.project_code) : "--"])
    entries.push(["创建时间", data.created_at ?? "--"])
    entries.push(["更新时间", data.updated_at ?? "--"])
    if (data.last_health_check) {
      entries.push(["最近健康检查", data.last_health_check])
    }
    if (data.health_url) {
      entries.push(["Health URL", data.health_url])
    }
    return entries
  }, [data])

  if (!open || !siteId) return null

  const handleCreateTask = async () => {
    if (!siteId) return
    setCreatingTask(true)
    setMessage(null)
    try {
      const response = await createDeploymentSiteTask({
        site_id: siteId,
        task_type: taskType,
        task_name: taskName.trim() || undefined,
        priority: priority as any,
      })
      if (response.error) {
        throw new Error(response.error)
      }
      setMessageTone("success")
      setMessage(response.message ?? "任务创建成功")
    } catch (err) {
      setMessageTone("error")
      setMessage(err instanceof Error ? err.message : "任务创建失败")
    } finally {
      setCreatingTask(false)
    }
  }

  const handleHealthCheck = async () => {
    if (!siteId) return
    setCheckingHealth(true)
    setMessage(null)
    try {
      const response = await healthcheckDeploymentSite(siteId)
      if (response.error) {
        throw new Error(response.error)
      }
      setMessageTone(response.healthy ? "success" : "error")
      setMessage(response.healthy ? "健康检查通过" : "目标服务未响应或失败")
      refresh()
    } catch (err) {
      setMessageTone("error")
      setMessage(err instanceof Error ? err.message : "健康检查失败")
    } finally {
      setCheckingHealth(false)
    }
  }

  const handleExportConfig = async () => {
    if (!siteId) return
    setExporting(true)
    try {
      const response = await exportDeploymentSiteConfig(siteId)
      if (response.error || !response.config) {
        throw new Error(response.error ?? "导出失败")
      }
      const blob = new Blob([JSON.stringify(response.config, null, 2)], {
        type: "application/json",
      })
      const url = URL.createObjectURL(blob)
      const link = document.createElement("a")
      link.href = url
      link.download = `${(response.name || data?.name || siteId).replace(/\s+/g, "_")}-config.json`
      document.body.appendChild(link)
      link.click()
      link.remove()
      URL.revokeObjectURL(url)
      setMessageTone("success")
      setMessage("配置导出成功")
    } catch (err) {
      setMessageTone("error")
      setMessage(err instanceof Error ? err.message : "导出配置失败")
    } finally {
      setExporting(false)
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex">
      <div className="absolute inset-0 bg-black/40" onClick={() => onOpenChange(false)} />
      <aside className="relative ml-auto flex h-full w-full max-w-xl flex-col border-l border-border bg-card shadow-xl">
        <header className="flex items-start justify-between border-b border-border px-6 py-4">
          <div>
            <h2 className="text-lg font-semibold text-foreground">{data?.name ?? siteId}</h2>
            <p className="text-xs text-muted-foreground">
              {data?.description ?? "查看部署站点的配置、历史及运行信息"}
            </p>
          </div>
          <Button variant="ghost" className="h-8 w-8" onClick={() => onOpenChange(false)}>
            ✕
          </Button>
        </header>

        {error && (
          <div className="mx-6 mt-4 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive">
            {error}
          </div>
        )}
        {loading && !data ? (
          <p className="mx-6 mt-4 text-sm text-muted-foreground">正在加载详情...</p>
        ) : null}

        <div className="flex-1 overflow-y-auto px-6 py-4">
          <div className="space-y-6">
            {infoEntries.length > 0 && (
              <section>
                <h3 className="text-sm font-semibold text-foreground">基本信息</h3>
                <div className="mt-2 space-y-2 text-xs">
                  {infoEntries.map(([label, value]) => (
                    <div key={label} className="flex items-start justify-between gap-4 border-b border-border/40 pb-2">
                      <span className="text-muted-foreground">{label}</span>
                      <span className="max-w-[60%] break-words text-foreground">{value}</span>
                    </div>
                  ))}
                </div>
              </section>
            )}

            {data?.config && (
              <section className="space-y-2">
                <div className="flex items-center justify-between">
                  <h3 className="text-sm font-semibold text-foreground">数据库配置</h3>
                  <Button variant="ghost" className="text-xs" onClick={handleExportConfig} disabled={exporting}>
                    {exporting ? "导出中..." : "导出配置"}
                  </Button>
                </div>
                <pre className="max-h-48 overflow-auto rounded border border-border/60 bg-muted/20 p-3 text-xs">
                  {JSON.stringify(data.config, null, 2)}
                </pre>
              </section>
            )}
          </div>
        </div>

        <footer className="border-t border-border px-6 py-4">
          {message && (
            <div
              className={cn("mb-2 rounded-md px-3 py-2 text-xs", {
                "bg-success/10 text-success": messageTone === "success",
                "bg-destructive/10 text-destructive": messageTone === "error",
                "bg-muted/20 text-muted-foreground": messageTone === "info",
              })}
            >
              {message}
            </div>
          )}
          <div className="space-y-3">
            <div className="grid gap-2 text-sm sm:grid-cols-2">
              <div>
                <label className="text-xs text-muted-foreground">任务类型</label>
                <select
                  value={taskType}
                  onChange={(event) => setTaskType(event.target.value as TaskType)}
                  className="mt-1 h-9 w-full rounded-md border border-input bg-background px-2 text-sm"
                >
                  {TASK_OPTIONS.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </div>
              <div>
                <label className="text-xs text-muted-foreground">优先级</label>
                <select
                  value={priority}
                  onChange={(event) => setPriority(event.target.value)}
                  className="mt-1 h-9 w-full rounded-md border border-input bg-background px-2 text-sm"
                >
                  {PRIORITY_OPTIONS.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </div>
              <div className="sm:col-span-2">
                <label className="text-xs text-muted-foreground">任务名称</label>
                <Input
                  value={taskName}
                  onChange={(event) => setTaskName(event.target.value)}
                  placeholder="默认将根据站点名称生成"
                  className="mt-1"
                />
              </div>
            </div>

            <div className="flex flex-wrap gap-2">
              <Button onClick={handleCreateTask} disabled={creatingTask}>
                {creatingTask ? "创建中..." : "创建任务"}
              </Button>
              <Button variant="outline" onClick={handleHealthCheck} disabled={checkingHealth}>
                {checkingHealth ? "检查中..." : "健康检查"}
              </Button>
              <Button variant="outline" onClick={handleExportConfig} disabled={exporting}>
                {exporting ? "导出中..." : "导出配置"}
              </Button>
            </div>

            {data?.status ? (
              <Badge className="self-start">当前状态：{data.status}</Badge>
            ) : null}
          </div>
        </footer>
      </aside>
    </div>
  )
}
