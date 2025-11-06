import { useCallback, useEffect, useMemo, useState } from "react"
import { FolderIcon, FileTextIcon, Loader2, RefreshCw } from "lucide-react"
import { cn } from "@/lib/utils"
import { buildApiUrl } from "@/lib/api"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Badge } from "@/components/ui/badge"

interface FileEntry {
  name: string
  path: string
  type: "file" | "directory"
  size: number | null
  modifiedAt: string
}

interface DirectoryListing {
  rootPath: string
  currentPath: string
  relativePath: string
  breadcrumbs: Array<{ name: string; path: string }>
  entries: FileEntry[]
}

interface FileBrowserPanelProps {
  siteId: string
  title?: string
}

function formatBytes(bytes: number | null) {
  if (bytes === null) return "-"
  if (bytes < 1024) return `${bytes} B`
  const units = ["KB", "MB", "GB", "TB"]
  let size = bytes / 1024
  let unitIndex = 0

  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024
    unitIndex += 1
  }

  return `${size.toFixed(size >= 10 ? 0 : 1)} ${units[unitIndex]}`
}

function formatModifiedTime(timestamp: string) {
  try {
    return new Intl.DateTimeFormat("zh-CN", {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    }).format(new Date(timestamp))
  } catch {
    return timestamp
  }
}

export function FileBrowserPanel({ siteId, title = "项目文件浏览" }: FileBrowserPanelProps) {
  const [directory, setDirectory] = useState<DirectoryListing | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [currentPath, setCurrentPath] = useState<string | null>(null)

  const resolveUrl = useCallback(
    (targetPath?: string) => {
      const base = buildApiUrl(`/api/deployment-sites/${siteId}/browse-directory`)
      const url =
        typeof window === "undefined"
          ? new URL(base, "http://localhost")
          : base.startsWith("http")
            ? new URL(base)
            : new URL(base, window.location.origin)
      if (targetPath) {
        url.searchParams.set("path", targetPath)
      }
      return url.toString()
    },
    [siteId]
  )

  const loadDirectory = useCallback(
    async (targetPath?: string) => {
      if (!siteId) return

      setLoading(true)
      setError(null)

      try {
        const response = await fetch(resolveUrl(targetPath), {
          method: "GET",
          cache: "no-store",
        })

        if (!response.ok) {
          const payload = await response.json().catch(() => null)
          throw new Error(payload?.error ?? "目录读取失败")
        }

        const data = (await response.json()) as DirectoryListing
        setDirectory(data)
        setCurrentPath(data.currentPath)
      } catch (err) {
        setError(err instanceof Error ? err.message : "目录读取失败")
      } finally {
        setLoading(false)
      }
    },
    [resolveUrl, siteId]
  )

  useEffect(() => {
    if (!siteId) {
      setDirectory(null)
      setCurrentPath(null)
      setError("未指定站点，无法浏览文件。")
      return
    }

    loadDirectory(undefined)
  }, [siteId, loadDirectory])

  const entries = useMemo(() => directory?.entries ?? [], [directory])

  const handleNavigate = useCallback(
    (target: string) => {
      if (loading) return
      loadDirectory(target)
    },
    [loadDirectory, loading]
  )

  return (
    <Card className="border-border">
      <CardHeader className="flex flex-row items-center justify-between space-y-0">
        <CardTitle className="text-lg font-semibold">{title}</CardTitle>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="gap-2"
          disabled={loading || !currentPath}
          onClick={() => currentPath && handleNavigate(currentPath)}
        >
          {loading ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
          刷新
        </Button>
      </CardHeader>
      <CardContent className="space-y-4">
        {error ? (
          <Alert variant="destructive">
            <AlertTitle>加载失败</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        ) : (
          <>
            <div className="flex flex-wrap items-center justify-between gap-3">
              <Breadcrumb>
                <BreadcrumbList>
                  {directory?.breadcrumbs.map((crumb, index) => {
                    const isLast = index === (directory?.breadcrumbs.length ?? 0) - 1
                    return (
                      <div key={crumb.path} className="flex items-center">
                        <BreadcrumbItem>
                          {isLast ? (
                            <BreadcrumbPage>{crumb.name}</BreadcrumbPage>
                          ) : (
                            <BreadcrumbLink
                              href="#"
                              onClick={(event) => {
                                event.preventDefault()
                                handleNavigate(crumb.path)
                              }}
                            >
                              {crumb.name}
                            </BreadcrumbLink>
                          )}
                        </BreadcrumbItem>
                        {!isLast && <BreadcrumbSeparator />}
                      </div>
                    )
                  })}
                </BreadcrumbList>
              </Breadcrumb>
              {directory?.relativePath ? (
                <Badge variant="secondary" className="font-mono">
                  {directory.relativePath}
                </Badge>
              ) : (
                <Badge variant="secondary">根目录</Badge>
              )}
            </div>

            <div className="overflow-hidden rounded-md border border-border">
              <div className="hidden grid-cols-[auto_auto_auto_auto] gap-3 bg-muted px-4 py-2 text-xs font-medium uppercase tracking-wide text-muted-foreground md:grid">
                <span>名称</span>
                <span>类型</span>
                <span>大小</span>
                <span>修改时间</span>
              </div>
              <div className="max-h-96 overflow-auto">
                {loading && (
                  <div className="flex items-center gap-2 px-4 py-6 text-sm text-muted-foreground">
                    <Loader2 className="h-4 w-4 animate-spin" />
                    正在加载目录...
                  </div>
                )}
                {!loading && entries.length === 0 && (
                  <div className="px-4 py-6 text-sm text-muted-foreground">目录为空。</div>
                )}
                {!loading &&
                  entries.map((entry) => (
                    <button
                      key={entry.path}
                      type="button"
                      className={cn(
                        "grid w-full grid-cols-1 items-center gap-2 border-t border-border px-4 py-3 text-left text-sm transition-colors hover:bg-muted md:grid-cols-[auto_auto_auto_auto]",
                        entry.type === "directory" ? "font-medium" : "font-normal"
                      )}
                      onClick={() => {
                        if (entry.type === "directory") {
                          handleNavigate(entry.path)
                        }
                      }}
                    >
                      <span className="flex items-center gap-2">
                        {entry.type === "directory" ? (
                          <FolderIcon className="h-4 w-4 text-blue-500" />
                        ) : (
                          <FileTextIcon className="h-4 w-4 text-muted-foreground" />
                        )}
                        {entry.name}
                      </span>
                      <span className="hidden text-sm text-muted-foreground md:block">
                        {entry.type === "directory" ? "目录" : "文件"}
                      </span>
                      <span className="hidden text-sm text-muted-foreground md:block">
                        {entry.type === "directory" ? "-" : formatBytes(entry.size)}
                      </span>
                      <span className="hidden text-sm text-muted-foreground md:block">
                        {formatModifiedTime(entry.modifiedAt)}
                      </span>
                    </button>
                  ))}
              </div>
            </div>
          </>
        )}
      </CardContent>
    </Card>
  )
}
