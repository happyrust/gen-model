"use client"

import { useState, useRef, useEffect } from "react"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Alert, AlertDescription } from "@/components/ui/alert"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { Badge } from "@/components/ui/badge"
import { Progress } from "@/components/ui/progress"
import { 
  Upload, 
  FileText, 
  Eye, 
  Download, 
  CheckCircle, 
  XCircle, 
  Info, 
  Loader2,
  RotateCcw,
  ZoomIn,
  ZoomOut,
  Move3D
} from "lucide-react"
import { toast } from "sonner"

interface XKTFileInfo {
  filename: string
  size: number
  version: number
  sections: number
  compressed: boolean
  metadata?: any
  geometryCount?: number
  entityCount?: number
  triangleCount?: number
}

interface XKTViewerProps {
  className?: string
}

export function XKTViewer({ className }: XKTViewerProps) {
  const [isUploading, setIsUploading] = useState(false)
  const [uploadProgress, setUploadProgress] = useState(0)
  const [fileInfo, setFileInfo] = useState<XKTFileInfo | null>(null)
  const [isAnalyzing, setIsAnalyzing] = useState(false)
  const [analysisResult, setAnalysisResult] = useState<any>(null)
  const [showViewer, setShowViewer] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const viewerRef = useRef<HTMLDivElement>(null)

  // 处理文件上传
  const handleFileUpload = async (file: File) => {
    if (!file.name.endsWith('.xkt')) {
      toast.error("请选择 .xkt 格式的文件")
      return
    }

    setIsUploading(true)
    setUploadProgress(0)

    try {
      // 模拟上传进度
      const progressInterval = setInterval(() => {
        setUploadProgress(prev => {
          if (prev >= 90) {
            clearInterval(progressInterval)
            return 90
          }
          return prev + 10
        })
      }, 100)

      // 分析文件
      const fileBuffer = await file.arrayBuffer()
      const analysis = await analyzeXKTFile(fileBuffer)
      
      clearInterval(progressInterval)
      setUploadProgress(100)

      const fileInfo: XKTFileInfo = {
        filename: file.name,
        size: file.size,
        version: analysis.version,
        sections: analysis.sections,
        compressed: analysis.compressed,
        metadata: analysis.metadata,
        geometryCount: analysis.geometryCount,
        entityCount: analysis.entityCount,
        triangleCount: analysis.triangleCount
      }

      setFileInfo(fileInfo)
      setAnalysisResult(analysis)
      toast.success(`文件分析完成: ${file.name}`)

    } catch (error: any) {
      console.error("文件分析失败:", error)
      toast.error(error.message || "文件分析失败")
    } finally {
      setIsUploading(false)
      setUploadProgress(0)
    }
  }

  // 分析 XKT 文件
  const analyzeXKTFile = async (buffer: ArrayBuffer): Promise<any> => {
    const dataView = new DataView(buffer)
    
    // 读取版本号 (前4字节，小端序)
    const version = dataView.getUint32(0, true)
    
    // 读取段数量 (第5-8字节)
    const sections = dataView.getUint32(4, true)
    
    // 读取段偏移表
    const offsets: number[] = []
    for (let i = 0; i < sections; i++) {
      offsets.push(dataView.getUint32(8 + i * 4, true))
    }
    
    // 检查压缩状态 (通过段偏移判断)
    const compressed = offsets.some(offset => offset > buffer.byteLength)
    
    // 尝试解析元数据
    let metadata = null
    if (offsets[0] > 0 && offsets[0] < buffer.byteLength) {
      try {
        const metadataBytes = new Uint8Array(buffer, offsets[0], Math.min(1000, buffer.byteLength - offsets[0]))
        const metadataText = new TextDecoder().decode(metadataBytes)
        if (metadataText.includes('{')) {
          const jsonStart = metadataText.indexOf('{')
          const jsonEnd = metadataText.lastIndexOf('}') + 1
          if (jsonEnd > jsonStart) {
            metadata = JSON.parse(metadataText.substring(jsonStart, jsonEnd))
          }
        }
      } catch (error) {
        console.warn("元数据解析失败:", error)
      }
    }
    
    // 估算几何数据
    let geometryCount = 0
    let entityCount = 0
    let triangleCount = 0
    
    if (offsets[1] > 0 && offsets[2] > 0) {
      // 估算三角形数量 (基于索引数据大小)
      const indexDataSize = offsets[2] - offsets[1]
      triangleCount = Math.floor(indexDataSize / 12) // 每个三角形3个索引，每个索引4字节
    }
    
    return {
      version,
      sections,
      compressed,
      metadata,
      geometryCount,
      entityCount,
      triangleCount,
      offsets: offsets.slice(0, 10) // 只返回前10个偏移
    }
  }

  // 处理拖拽上传
  const handleDrop = (e: React.DragEvent) => {
    e.preventDefault()
    const files = Array.from(e.dataTransfer.files)
    if (files.length > 0) {
      handleFileUpload(files[0])
    }
  }

  const handleDragOver = (e: React.DragEvent) => {
    e.preventDefault()
  }

  // 打开 3D 查看器
  const openViewer = () => {
    setShowViewer(true)
    toast.info("正在加载 3D 查看器...")
  }

  // 下载文件
  const downloadFile = () => {
    if (fileInputRef.current?.files?.[0]) {
      const link = document.createElement("a")
      link.href = URL.createObjectURL(fileInputRef.current.files[0])
      link.download = fileInfo?.filename || "model.xkt"
      document.body.appendChild(link)
      link.click()
      document.body.removeChild(link)
      toast.success("文件下载开始")
    }
  }

  // 格式化文件大小
  const formatFileSize = (bytes: number): string => {
    if (bytes === 0) return "0 B"
    const k = 1024
    const sizes = ["B", "KB", "MB", "GB"]
    const i = Math.floor(Math.log(bytes) / Math.log(k))
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + " " + sizes[i]
  }

  return (
    <div className={className}>
      <Card className="w-full">
        <CardHeader>
          <div className="flex items-center justify-between">
            <div className="space-y-1">
              <CardTitle className="text-2xl">XKT 文件测试查看器</CardTitle>
              <CardDescription>
                上传、分析和预览 XKT 格式的 3D 模型文件
              </CardDescription>
            </div>
            <Eye className="h-8 w-8 text-muted-foreground" />
          </div>
        </CardHeader>
        <CardContent className="space-y-6">
          <Tabs defaultValue="upload" className="w-full">
            <TabsList className="grid w-full grid-cols-3">
              <TabsTrigger value="upload">文件上传</TabsTrigger>
              <TabsTrigger value="analysis">文件分析</TabsTrigger>
              <TabsTrigger value="viewer">3D 预览</TabsTrigger>
            </TabsList>

            {/* 文件上传标签页 */}
            <TabsContent value="upload" className="space-y-4">
              <div
                className="border-2 border-dashed border-muted-foreground/25 rounded-lg p-8 text-center hover:border-muted-foreground/50 transition-colors"
                onDrop={handleDrop}
                onDragOver={handleDragOver}
              >
                <Upload className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
                <h3 className="text-lg font-semibold mb-2">拖拽上传 XKT 文件</h3>
                <p className="text-muted-foreground mb-4">
                  或者点击下方按钮选择文件
                </p>
                <input
                  ref={fileInputRef}
                  type="file"
                  accept=".xkt"
                  onChange={(e) => {
                    if (e.target.files?.[0]) {
                      handleFileUpload(e.target.files[0])
                    }
                  }}
                  className="hidden"
                />
                <Button
                  onClick={() => fileInputRef.current?.click()}
                  disabled={isUploading}
                  className="mb-4"
                >
                  {isUploading ? (
                    <>
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                      分析中...
                    </>
                  ) : (
                    <>
                      <FileText className="mr-2 h-4 w-4" />
                      选择文件
                    </>
                  )}
                </Button>
                
                {isUploading && (
                  <div className="w-full max-w-xs mx-auto">
                    <Progress value={uploadProgress} className="mb-2" />
                    <p className="text-sm text-muted-foreground">
                      分析进度: {uploadProgress}%
                    </p>
                  </div>
                )}
              </div>

              <Alert>
                <Info className="h-4 w-4" />
                <AlertDescription>
                  支持 XKT v10 格式文件，文件大小建议不超过 100MB
                </AlertDescription>
              </Alert>
            </TabsContent>

            {/* 文件分析标签页 */}
            <TabsContent value="analysis" className="space-y-4">
              {!fileInfo ? (
                <div className="text-center py-8 text-muted-foreground">
                  <FileText className="h-12 w-12 mx-auto mb-4" />
                  <p>请先上传 XKT 文件进行分析</p>
                </div>
              ) : (
                <div className="space-y-4">
                  <div className="grid gap-4 md:grid-cols-2">
                    <Card>
                      <CardHeader className="pb-3">
                        <CardTitle className="text-lg">基本信息</CardTitle>
                      </CardHeader>
                      <CardContent className="space-y-2">
                        <div className="flex justify-between">
                          <span className="text-muted-foreground">文件名:</span>
                          <span className="font-medium">{fileInfo.filename}</span>
                        </div>
                        <div className="flex justify-between">
                          <span className="text-muted-foreground">文件大小:</span>
                          <span className="font-medium">{formatFileSize(fileInfo.size)}</span>
                        </div>
                        <div className="flex justify-between">
                          <span className="text-muted-foreground">版本:</span>
                          <Badge variant={fileInfo.version === 10 ? "default" : "destructive"}>
                            v{fileInfo.version}
                          </Badge>
                        </div>
                        <div className="flex justify-between">
                          <span className="text-muted-foreground">段数量:</span>
                          <span className="font-medium">{fileInfo.sections}</span>
                        </div>
                        <div className="flex justify-between">
                          <span className="text-muted-foreground">压缩状态:</span>
                          <Badge variant={fileInfo.compressed ? "secondary" : "outline"}>
                            {fileInfo.compressed ? "已压缩" : "未压缩"}
                          </Badge>
                        </div>
                      </CardContent>
                    </Card>

                    <Card>
                      <CardHeader className="pb-3">
                        <CardTitle className="text-lg">几何数据</CardTitle>
                      </CardHeader>
                      <CardContent className="space-y-2">
                        <div className="flex justify-between">
                          <span className="text-muted-foreground">几何体数量:</span>
                          <span className="font-medium">{fileInfo.geometryCount || "未知"}</span>
                        </div>
                        <div className="flex justify-between">
                          <span className="text-muted-foreground">实体数量:</span>
                          <span className="font-medium">{fileInfo.entityCount || "未知"}</span>
                        </div>
                        <div className="flex justify-between">
                          <span className="text-muted-foreground">三角形数量:</span>
                          <span className="font-medium">{fileInfo.triangleCount || "未知"}</span>
                        </div>
                      </CardContent>
                    </Card>
                  </div>

                  {fileInfo.metadata && (
                    <Card>
                      <CardHeader className="pb-3">
                        <CardTitle className="text-lg">元数据</CardTitle>
                      </CardHeader>
                      <CardContent>
                        <pre className="bg-muted p-4 rounded-lg text-sm overflow-auto max-h-40">
                          {JSON.stringify(fileInfo.metadata, null, 2)}
                        </pre>
                      </CardContent>
                    </Card>
                  )}

                  <div className="flex gap-2">
                    <Button onClick={openViewer} className="flex-1">
                      <Eye className="mr-2 h-4 w-4" />
                      打开 3D 查看器
                    </Button>
                    <Button variant="outline" onClick={downloadFile}>
                      <Download className="mr-2 h-4 w-4" />
                      下载文件
                    </Button>
                  </div>
                </div>
              )}
            </TabsContent>

            {/* 3D 预览标签页 */}
            <TabsContent value="viewer" className="space-y-4">
              {!fileInfo ? (
                <div className="text-center py-8 text-muted-foreground">
                  <Eye className="h-12 w-12 mx-auto mb-4" />
                  <p>请先上传并分析 XKT 文件</p>
                </div>
              ) : (
                <div className="space-y-4">
                  <div className="flex items-center justify-between">
                    <h3 className="text-lg font-semibold">3D 模型预览</h3>
                    <div className="flex gap-2">
                      <Button size="sm" variant="outline">
                        <RotateCcw className="h-4 w-4" />
                      </Button>
                      <Button size="sm" variant="outline">
                        <ZoomIn className="h-4 w-4" />
                      </Button>
                      <Button size="sm" variant="outline">
                        <ZoomOut className="h-4 w-4" />
                      </Button>
                      <Button size="sm" variant="outline">
                        <Move3D className="h-4 w-4" />
                      </Button>
                    </div>
                  </div>
                  
                  <div 
                    ref={viewerRef}
                    className="w-full h-96 bg-muted rounded-lg flex items-center justify-center border"
                  >
                    <div className="text-center">
                      <Move3D className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
                      <p className="text-muted-foreground mb-4">3D 查看器将在此处显示</p>
                      <Button onClick={openViewer}>
                        <Eye className="mr-2 h-4 w-4" />
                        启动 3D 查看器
                      </Button>
                    </div>
                  </div>
                </div>
              )}
            </TabsContent>
          </Tabs>

          {/* 3D 查看器对话框 */}
          <Dialog open={showViewer} onOpenChange={setShowViewer}>
            <DialogContent className="max-w-6xl h-[80vh]">
              <DialogHeader>
                <DialogTitle>3D 模型查看器</DialogTitle>
                <DialogDescription>
                  {fileInfo?.filename} - {formatFileSize(fileInfo?.size || 0)}
                </DialogDescription>
              </DialogHeader>
              <div className="flex-1 bg-muted rounded-lg flex items-center justify-center">
                <div className="text-center">
                  <Move3D className="h-16 w-16 mx-auto mb-4 text-muted-foreground" />
                  <h3 className="text-lg font-semibold mb-2">3D 查看器</h3>
                  <p className="text-muted-foreground mb-4">
                    基于 xeokit 的 3D 查看器将在此处加载模型
                  </p>
                  <div className="flex gap-2 justify-center">
                    <Button variant="outline" size="sm">
                      <RotateCcw className="mr-2 h-4 w-4" />
                      重置视角
                    </Button>
                    <Button variant="outline" size="sm">
                      <ZoomIn className="mr-2 h-4 w-4" />
                      放大
                    </Button>
                    <Button variant="outline" size="sm">
                      <ZoomOut className="mr-2 h-4 w-4" />
                      缩小
                    </Button>
                  </div>
                </div>
              </div>
            </DialogContent>
          </Dialog>
        </CardContent>
      </Card>
    </div>
  )
}


