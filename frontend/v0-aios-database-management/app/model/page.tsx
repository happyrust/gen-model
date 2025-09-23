"use client"

import { useEffect, useMemo, useState } from "react"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { fetchDatabases, generateObjModel } from "@/lib/api"
import type { DatabaseInfo } from "@/types/api"

export default function ModelPage() {
  const [databases, setDatabases] = useState<DatabaseInfo[]>([])
  const [loadingDbs, setLoadingDbs] = useState(true)
  const [mode, setMode] = useState<"dbnum" | "refno">("dbnum")
  const [selectedDb, setSelectedDb] = useState<string>("")
  const [refno, setRefno] = useState("")
  const [submitting, setSubmitting] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [messageTone, setMessageTone] = useState<"success" | "error" | "info">("info")
  const [downloadUrl, setDownloadUrl] = useState<string | null>(null)

  useEffect(() => {
    async function load() {
      try {
        const data = await fetchDatabases()
        setDatabases(data)
      } catch (err) {
        console.error(err)
      } finally {
        setLoadingDbs(false)
      }
    }
    void load()
  }, [])

  const sortedDatabases = useMemo(
    () => [...databases].sort((a, b) => a.db_num - b.db_num),
    [databases],
  )

  async function handleSubmit(event: React.FormEvent) {
    event.preventDefault()
    setSubmitting(true)
    setMessage(null)
    setDownloadUrl(null)
    try {
      const payload = mode === "dbnum"
        ? { dbnum: selectedDb ? Number(selectedDb) : undefined }
        : { refno: refno.trim() || undefined }

      if (!payload.dbnum && !payload.refno) {
        setMessageTone("error")
        setMessage("请先选择一个 dbnum 或输入 refno")
        return
      }

      const response = await generateObjModel(payload)
      if (response.error) {
        throw new Error(response.error)
      }

      const url = response.download_url ?? (response.filename ? `/api/model/download/${response.filename}` : null)
      setDownloadUrl(url)
      setMessageTone("success")
      setMessage(response.message ?? "模型生成成功，点击下方链接下载 OBJ 文件")
    } catch (err) {
      setMessageTone("error")
      setMessage(err instanceof Error ? err.message : "模型生成失败")
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="mx-auto w-full max-w-4xl space-y-6 px-6 py-10">
      <header className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold text-foreground">生成模型</h1>
          <p className="text-sm text-muted-foreground">选择 dbnum 或 refno，后台将在完成后提供 OBJ 下载。</p>
        </div>
        <Badge className="bg-primary/10 text-primary">OBJ 导出</Badge>
      </header>

      <Card>
        <CardHeader>
          <CardTitle className="text-lg">生成设置</CardTitle>
        </CardHeader>
        <CardContent>
          <form className="space-y-4" onSubmit={handleSubmit}>
            <div className="flex gap-3 text-sm">
              <button
                type="button"
                className={`rounded-md border px-3 py-1.5 ${mode === "dbnum" ? "border-primary text-primary" : "border-border text-muted-foreground"}`}
                onClick={() => setMode("dbnum")}
              >
                按 dbnum 选择
              </button>
              <button
                type="button"
                className={`rounded-md border px-3 py-1.5 ${mode === "refno" ? "border-primary text-primary" : "border-border text-muted-foreground"}`}
                onClick={() => setMode("refno")}
              >
                按 refno 输入
              </button>
            </div>

            {mode === "dbnum" ? (
              <div className="space-y-2">
                <label className="text-sm font-medium text-foreground">选择数据库编号</label>
                {loadingDbs ? (
                  <p className="text-xs text-muted-foreground">正在加载数据库列表...</p>
                ) : (
                  <select
                    className="h-10 w-full rounded-md border border-input bg-background px-2 text-sm"
                    value={selectedDb}
                    onChange={(event) => setSelectedDb(event.target.value)}
                  >
                    <option value="">请选择 dbnum</option>
                    {sortedDatabases.map((db) => (
                      <option key={db.db_num} value={db.db_num}>
                        {db.db_num} — {db.name}
                      </option>
                    ))}
                  </select>
                )}
              </div>
            ) : (
              <div className="space-y-2">
                <label className="text-sm font-medium text-foreground">输入 refno</label>
                <Input
                  value={refno}
                  onChange={(event) => setRefno(event.target.value)}
                  placeholder="例如：24383/92720"
                />
                <p className="text-xs text-muted-foreground">支持单个 refno；如需批量，可在后台任务中处理。</p>
              </div>
            )}

            <div className="flex justify-end gap-2 pt-2">
              <Button type="submit" disabled={submitting}>
                {submitting ? "生成中..." : "生成 OBJ"}
              </Button>
            </div>
          </form>

          {message && (
            <div
              className={`mt-4 rounded-md border px-3 py-2 text-sm ${
                messageTone === "success"
                  ? "border-success/60 bg-success/10 text-success"
                  : messageTone === "error"
                  ? "border-destructive/60 bg-destructive/10 text-destructive"
                  : "border-border/60 bg-muted/20 text-muted-foreground"
              }`}
            >
              {message}
            </div>
          )}

          {downloadUrl && (
            <div className="mt-4">
              <a
                href={downloadUrl}
                className="inline-flex items-center rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
              >
                下载 OBJ 文件
              </a>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
