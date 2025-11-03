"use client"

import { useState } from "react"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Switch } from "@/components/ui/switch"
import { Alert, AlertDescription } from "@/components/ui/alert"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { Badge } from "@/components/ui/badge"
import { Loader2, Download, Eye, Package, CheckCircle, XCircle, Info, AlertCircle } from "lucide-react"
import { toast } from "sonner"
import { buildXktApiUrl, resolveXktResourceUrl } from "@/lib/xkt-api"

interface XKTFile {
  filename: string
  size: number
  url: string
  dbno: number
  refno?: string
  timestamp: string
}

interface GenerationParams {
  dbno: string
  refno: string
  compress: boolean
}

export function XKTGenerator() {
  const [isGenerating, setIsGenerating] = useState(false)
  const [params, setParams] = useState<GenerationParams>({
    dbno: "1112",
    refno: "",
    compress: true
  })
  const [generatedFiles, setGeneratedFiles] = useState<XKTFile[]>([])
  const [selectedFile, setSelectedFile] = useState<XKTFile | null>(null)
  const [error, setError] = useState<string | null>(null)

  const generateXKT = async () => {
    if (!params.dbno) {
      const message = "请输入数据库号"
      toast.error(message)
      setError(message)
      return
    }

    const dbNumber = Number.parseInt(params.dbno, 10)
    if (Number.isNaN(dbNumber)) {
      const message = "数据库号格式不正确"
      toast.error(message)
      setError(message)
      return
    }

    setIsGenerating(true)
    setError(null)

    try {
      const response = await fetch(buildXktApiUrl("/api/xkt/generate"), {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          dbno: dbNumber,
          refno: params.refno || undefined,
          compress: params.compress
        })
      })

      if (!response.ok) {
        const error = await response.text()
        throw new Error(error || `生成失败 (HTTP ${response.status})`)
      }

      const result = await response.json()

      if (result.success) {
        const downloadUrl = resolveXktResourceUrl(result.url)

        if (!downloadUrl) {
          throw new Error("未返回有效的下载地址")
        }

        const newFile: XKTFile = {
          filename: result.filename,
          size: 0,
          url: downloadUrl,
          dbno: dbNumber,
          refno: params.refno || undefined,
          timestamp: new Date().toISOString()
        }

        // 获取文件大小
        try {
          const fileResponse = await fetch(downloadUrl)
          if (fileResponse.ok) {
            const blob = await fileResponse.blob()
            newFile.size = blob.size
          }
        } catch (error) {
          console.error("获取文件大小失败:", error)
        }

        setGeneratedFiles(prev => [newFile, ...prev])
        toast.success(`XKT文件生成成功: ${result.filename}`)
        setError(null)
      } else {
        throw new Error(result?.message || "生成XKT文件失败")
      }
    } catch (error: any) {
      console.error("生成XKT失败:", error)
      const message = error instanceof Error ? error.message : "生成XKT文件失败"
      toast.error(message)
      setError(message)
    } finally {
      setIsGenerating(false)
    }
  }

  const downloadFile = (file: XKTFile) => {
    if (!file.url) {
      const message = "暂无可用的下载地址"
      toast.error(message)
      setError(message)
      return
    }

    const link = document.createElement("a")
    link.href = file.url
    link.download = file.filename
    document.body.appendChild(link)
    link.click()
    document.body.removeChild(link)
    toast.success(`开始下载: ${file.filename}`)
  }

  const viewFile = (file: XKTFile) => {
    setSelectedFile(file)
    toast.info(`准备查看: ${file.filename}`)
  }

  const formatFileSize = (bytes: number): string => {
    if (bytes === 0) return "未知"
    const k = 1024
    const sizes = ["B", "KB", "MB", "GB"]
    const i = Math.floor(Math.log(bytes) / Math.log(k))
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + " " + sizes[i]
  }

  return (
    <Card className="w-full">
      <CardHeader>
        <div className="flex items-center justify-between">
          <div className="space-y-1">
            <CardTitle className="text-2xl">XKT 模型生成器</CardTitle>
            <CardDescription>
              生成用于3D查看器的XKT格式文件
            </CardDescription>
          </div>
          <Package className="h-8 w-8 text-muted-foreground" />
        </div>
      </CardHeader>
      <CardContent className="space-y-6">
        <Tabs defaultValue="generate" className="w-full">
          <TabsList className="grid w-full grid-cols-2">
            <TabsTrigger value="generate">生成XKT</TabsTrigger>
            <TabsTrigger value="history">历史记录</TabsTrigger>
          </TabsList>

          <TabsContent value="generate" className="space-y-4">
            <div className="grid gap-4">
              <div className="grid gap-2">
                <Label htmlFor="dbno">数据库号 *</Label>
                <Input
                  id="dbno"
                  type="number"
                  placeholder="例如: 1112"
                  value={params.dbno}
                  onChange={(e) => setParams(prev => ({ ...prev, dbno: e.target.value }))}
                  disabled={isGenerating}
                />
                <p className="text-xs text-muted-foreground">
                  必填项，输入数据库编号
                </p>
              </div>

              <div className="grid gap-2">
                <Label htmlFor="refno">参考号（可选）</Label>
                <Input
                  id="refno"
                  type="text"
                  placeholder="例如: 17496/266203"
                  value={params.refno}
                  onChange={(e) => setParams(prev => ({ ...prev, refno: e.target.value }))}
                  disabled={isGenerating}
                />
                <p className="text-xs text-muted-foreground">
                  可选项，留空则生成整个数据库
                </p>
              </div>

              <div className="flex items-center justify-between">
                <div className="space-y-0.5">
                  <Label htmlFor="compress">压缩文件</Label>
                  <p className="text-xs text-muted-foreground">
                    启用压缩可减少文件大小约75%
                  </p>
                </div>
                <Switch
                  id="compress"
                  checked={params.compress}
                  onCheckedChange={(checked) => setParams(prev => ({ ...prev, compress: checked }))}
                  disabled={isGenerating}
                />
              </div>

              <Alert>
                <Info className="h-4 w-4" />
                <AlertDescription>
                  生成的XKT文件可直接在3D查看器中加载显示
                </AlertDescription>
              </Alert>

              {error && (
                <Alert variant="destructive">
                  <AlertCircle className="h-4 w-4" />
                  <AlertDescription>{error}</AlertDescription>
                </Alert>
              )}

              <Button
                onClick={generateXKT}
                disabled={isGenerating || !params.dbno}
                className="w-full"
                size="lg"
              >
                {isGenerating ? (
                  <>
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                    正在生成...
                  </>
                ) : (
                  <>
                    <Package className="mr-2 h-4 w-4" />
                    生成XKT文件
                  </>
                )}
              </Button>
            </div>
          </TabsContent>

          <TabsContent value="history" className="space-y-4">
            {generatedFiles.length === 0 ? (
              <div className="text-center py-8 text-muted-foreground">
                暂无生成记录
              </div>
            ) : (
              <div className="space-y-3">
                {generatedFiles.map((file, index) => (
                  <div
                    key={index}
                    className="flex items-center justify-between p-3 border rounded-lg hover:bg-accent transition-colors"
                  >
                    <div className="flex-1 space-y-1">
                      <div className="flex items-center gap-2">
                        <CheckCircle className="h-4 w-4 text-green-500" />
                        <span className="font-medium text-sm">{file.filename}</span>
                        {file.refno && (
                          <Badge variant="secondary" className="text-xs">
                            {file.refno}
                          </Badge>
                        )}
                      </div>
                      <div className="flex items-center gap-4 text-xs text-muted-foreground">
                        <span>DB: {file.dbno}</span>
                        <span>大小: {formatFileSize(file.size)}</span>
                        <span>{new Date(file.timestamp).toLocaleString()}</span>
                      </div>
                    </div>
                    <div className="flex items-center gap-2">
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => viewFile(file)}
                      >
                        <Eye className="h-4 w-4" />
                      </Button>
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => downloadFile(file)}
                      >
                        <Download className="h-4 w-4" />
                      </Button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </TabsContent>
        </Tabs>

        {/* 查看器对话框 */}
        <Dialog open={!!selectedFile} onOpenChange={(open) => !open && setSelectedFile(null)}>
          <DialogContent className="max-w-4xl">
            <DialogHeader>
              <DialogTitle>XKT文件查看器</DialogTitle>
              <DialogDescription>
                {selectedFile?.filename} - {formatFileSize(selectedFile?.size || 0)}
              </DialogDescription>
            </DialogHeader>
            <div className="min-h-[400px] bg-muted rounded-lg flex items-center justify-center">
              <p className="text-muted-foreground">
                3D查看器将在此处显示模型
              </p>
            </div>
          </DialogContent>
        </Dialog>
      </CardContent>
    </Card>
  )
}
