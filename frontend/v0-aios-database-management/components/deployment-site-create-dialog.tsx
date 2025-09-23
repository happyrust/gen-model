"use client"

import { useState } from "react"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Textarea } from "@/components/ui/textarea"
import { importDeploymentSite } from "@/lib/api"

type FormState = {
  path: string
  name: string
  description: string
  env: string
  owner: string
  notes: string
  health_url: string
}

interface DeploymentSiteCreateDialogProps {
  onCreated?: () => void
}

export function DeploymentSiteCreateDialog({ onCreated }: DeploymentSiteCreateDialogProps) {
  const [open, setOpen] = useState(false)
  const [loading, setLoading] = useState(false)
  const [form, setForm] = useState<FormState>({
    path: "",
    name: "",
    description: "",
    env: "",
    owner: "",
    notes: "",
    health_url: "",
  })

  function close() {
    if (loading) return
    setOpen(false)
  }

  async function handleSubmit(event: React.FormEvent) {
    event.preventDefault()
    if (!form.path.trim()) {
      alert("请输入 DbOption.toml 路径")
      return
    }
    setLoading(true)
    try {
      const payload = {
        path: form.path.trim(),
        name: form.name.trim() || undefined,
        description: form.description.trim() || undefined,
        env: form.env.trim() || undefined,
        owner: form.owner.trim() || undefined,
        notes: form.notes.trim() || undefined,
        health_url: form.health_url.trim() || undefined,
      }
      await importDeploymentSite(payload)
      alert("站点导入成功")
      setForm({ path: "", name: "", description: "", env: "", owner: "", notes: "", health_url: "" })
      setOpen(false)
      onCreated?.()
    } catch (error) {
      alert(error instanceof Error ? error.message : "创建站点失败")
    } finally {
      setLoading(false)
    }
  }

  return (
    <>
      <Button
        variant="secondary"
        className="gap-2"
        onClick={() => setOpen(true)}
        disabled={loading}
      >
        创建站点
      </Button>
      {open ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 px-4">
          <div className="w-full max-w-xl rounded-xl border border-border bg-card p-6 shadow-lg">
            <h2 className="text-lg font-semibold text-foreground">导入 DbOption.toml 创建站点</h2>
            <p className="mt-1 text-sm text-muted-foreground">
              填写 DbOption 路径及可选信息，系统将自动导入部署站点配置。
            </p>
            <form className="mt-4 space-y-4" onSubmit={handleSubmit}>
              <div>
                <label className="text-sm font-medium text-foreground">DbOption.toml 路径</label>
                <Input
                  required
                  value={form.path}
                  onChange={(event) => setForm((prev) => ({ ...prev, path: event.target.value }))}
                  placeholder="例如：/path/to/DbOption.toml"
                  className="mt-1"
                />
              </div>

              <div className="grid gap-4 sm:grid-cols-2">
                <div>
                  <label className="text-sm font-medium text-foreground">站点名称</label>
                  <Input
                    value={form.name}
                    onChange={(event) => setForm((prev) => ({ ...prev, name: event.target.value }))}
                    placeholder="可选，例如：项目A-dev"
                    className="mt-1"
                  />
                </div>
                <div>
                  <label className="text-sm font-medium text-foreground">环境</label>
                  <Input
                    value={form.env}
                    onChange={(event) => setForm((prev) => ({ ...prev, env: event.target.value }))}
                    placeholder="dev / staging / prod"
                    className="mt-1"
                  />
                </div>
                <div>
                  <label className="text-sm font-medium text-foreground">负责人</label>
                  <Input
                    value={form.owner}
                    onChange={(event) => setForm((prev) => ({ ...prev, owner: event.target.value }))}
                    placeholder="可选"
                    className="mt-1"
                  />
                </div>
                <div>
                  <label className="text-sm font-medium text-foreground">健康检查地址</label>
                  <Input
                    value={form.health_url}
                    onChange={(event) => setForm((prev) => ({ ...prev, health_url: event.target.value }))}
                    placeholder="可选，示例：http://mysite/health"
                    className="mt-1"
                  />
                </div>
              </div>

              <div>
                <label className="text-sm font-medium text-foreground">描述</label>
                <Textarea
                  value={form.description}
                  onChange={(event) => setForm((prev) => ({ ...prev, description: event.target.value }))}
                  placeholder="可选"
                  className="mt-1"
                />
              </div>

              <div>
                <label className="text-sm font-medium text-foreground">备注</label>
                <Input
                  value={form.notes}
                  onChange={(event) => setForm((prev) => ({ ...prev, notes: event.target.value }))}
                  placeholder="可选"
                  className="mt-1"
                />
              </div>

              <div className="flex justify-end gap-2 pt-2">
                <Button type="button" variant="outline" onClick={close} disabled={loading}>
                  取消
                </Button>
                <Button type="submit" disabled={loading}>
                  {loading ? "创建中..." : "立即创建"}
                </Button>
              </div>
            </form>
          </div>
        </div>
      ) : null}
    </>
  )
}
