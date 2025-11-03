"use client"

import { useState } from "react"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Alert, AlertDescription } from "@/components/ui/alert"
import { Badge } from "@/components/ui/badge"
import { Sidebar } from "@/components/sidebar"
import { Breadcrumb, BreadcrumbItem, BreadcrumbLink, BreadcrumbList, BreadcrumbSeparator } from "@/components/ui/breadcrumb"
import { 
  Upload, 
  FileText, 
  CheckCircle, 
  XCircle, 
  Info, 
  Loader2,
  Download,
  Eye
} from "lucide-react"
import { toast } from "sonner"

interface XKTTestResult {
  filename: string
  size: number
  version: number
  sections: number
  compressed: boolean
  valid: boolean
  errors: string[]
  metadata?: any
}

export default function XKTTestPage() {
  const [isTesting, setIsTesting] = useState(false)
  const [testResult, setTestResult] = useState<XKTTestResult | null>(null)
  const [selectedFile, setSelectedFile] = useState<File | null>(null)

  // 测试 XKT 文件
  const testXKTFile = async (file: File) => {
    setIsTesting(true)
    setTestResult(null)

    try {
      const buffer = await file.arrayBuffer()
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
      
      // 验证文件结构
      const errors: string[] = []
      let valid = true
      
      // 检查版本号
      if (version !== 10) {
        errors.push(`版本号错误: 期望 10，实际 ${version}`)
        valid = false
      }
      
      // 检查段数量
      if (sections !== 29) {
        errors.push(`段数量错误: 期望 29，实际 ${sections}`)
        valid = false
      }
      
      // 检查段偏移
      for (let i = 0; i < offsets.length; i++) {
        if (offsets[i] > buffer.byteLength) {
          errors.push(`段 ${i} 偏移超出文件大小: ${offsets[i]} > ${buffer.byteLength}`)
          valid = false
        }
      }
      
      // 检查段偏移顺序
      for (let i = 1; i < offsets.length; i++) {
        if (offsets[i] > 0 && offsets[i-1] > 0 && offsets[i] < offsets[i-1]) {
          errors.push(`段偏移顺序错误: 段 ${i} (${offsets[i]}) < 段 ${i-1} (${offsets[i-1]})`)
          valid = false
        }
      }
      
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
          errors.push("元数据解析失败")
        }
      }
      
      // 检查压缩状态
      const compressed = offsets.some(offset => offset > buffer.byteLength)
      
      const result: XKTTestResult = {
        filename: file.name,
        size: file.size,
        version,
        sections,
        compressed,
        valid,
        errors,
        metadata
      }
      
      setTestResult(result)
      
      if (valid) {
        toast.success("XKT 文件验证通过！")
      } else {
        toast.error(`XKT 文件验证失败: ${errors.length} 个错误`)
      }
      
    } catch (error: any) {
      console.error("文件测试失败:", error)
      toast.error(error.message || "文件测试失败")
    } finally {
      setIsTesting(false)
    }
  }

  // 处理文件选择
  const handleFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0]
    if (file) {
      setSelectedFile(file)
      testXKTFile(file)
    }
  }

  // 下载测试文件
  const downloadTestFile = () => {
    if (selectedFile) {
      const link = document.createElement("a")
      link.href = URL.createObjectURL(selectedFile)
      link.download = selectedFile.name
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
    <div className="min-h-screen bg-background">
      {/* 侧边栏 */}
      <Sidebar />

      {/* 主内容区域 */}
      <div className="ml-64 p-8">
        {/* 页面头部 */}
        <div className="mb-8">
          <Breadcrumb className="mb-4">
            <BreadcrumbList>
              <BreadcrumbItem>
                <BreadcrumbLink href="/">主页</BreadcrumbLink>
              </BreadcrumbItem>
              <BreadcrumbSeparator />
              <BreadcrumbItem>
                <BreadcrumbLink>工具</BreadcrumbLink>
              </BreadcrumbItem>
              <BreadcrumbSeparator />
              <BreadcrumbItem>
                <BreadcrumbLink>XKT 测试</BreadcrumbLink>
              </BreadcrumbItem>
            </BreadcrumbList>
          </Breadcrumb>

          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-3xl font-bold text-foreground mb-2">
                XKT 文件测试工具
              </h1>
              <p className="text-lg text-muted-foreground">
                验证 XKT v10 格式文件的正确性和完整性
              </p>
            </div>
            <Badge className="bg-primary text-primary-foreground px-4 py-2">
              v10 标准验证
            </Badge>
          </div>
        </div>

        <div className="grid gap-6 md:grid-cols-2">
          {/* 文件上传区域 */}
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2">
                <Upload className="h-5 w-5" />
                文件上传
              </CardTitle>
              <CardDescription>
                选择要测试的 XKT 文件
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="space-y-2">
                <Label htmlFor="file-input">选择 XKT 文件</Label>
                <Input
                  id="file-input"
                  type="file"
                  accept=".xkt"
                  onChange={handleFileSelect}
                  disabled={isTesting}
                />
                <p className="text-xs text-muted-foreground">
                  支持 .xkt 格式文件，建议文件大小不超过 100MB
                </p>
              </div>

              {isTesting && (
                <div className="flex items-center gap-2 text-sm text-muted-foreground">
                  <Loader2 className="h-4 w-4 animate-spin" />
                  正在分析文件...
                </div>
              )}

              <Alert>
                <Info className="h-4 w-4" />
                <AlertDescription>
                  测试工具将验证 XKT v10 格式的版本号、段结构、偏移表等关键信息
                </AlertDescription>
              </Alert>
            </CardContent>
          </Card>

          {/* 测试结果区域 */}
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2">
                <FileText className="h-5 w-5" />
                测试结果
              </CardTitle>
              <CardDescription>
                文件验证结果和详细信息
              </CardDescription>
            </CardHeader>
            <CardContent>
              {!testResult ? (
                <div className="text-center py-8 text-muted-foreground">
                  <FileText className="h-12 w-12 mx-auto mb-4" />
                  <p>请先选择 XKT 文件进行测试</p>
                </div>
              ) : (
                <div className="space-y-4">
                  {/* 验证状态 */}
                  <div className="flex items-center gap-2">
                    {testResult.valid ? (
                      <CheckCircle className="h-5 w-5 text-green-500" />
                    ) : (
                      <XCircle className="h-5 w-5 text-red-500" />
                    )}
                    <span className="font-medium">
                      {testResult.valid ? "验证通过" : "验证失败"}
                    </span>
                    <Badge variant={testResult.valid ? "default" : "destructive"}>
                      {testResult.valid ? "有效" : "无效"}
                    </Badge>
                  </div>

                  {/* 基本信息 */}
                  <div className="grid gap-2 text-sm">
                    <div className="flex justify-between">
                      <span className="text-muted-foreground">文件名:</span>
                      <span className="font-medium">{testResult.filename}</span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-muted-foreground">文件大小:</span>
                      <span className="font-medium">{formatFileSize(testResult.size)}</span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-muted-foreground">版本:</span>
                      <Badge variant={testResult.version === 10 ? "default" : "destructive"}>
                        v{testResult.version}
                      </Badge>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-muted-foreground">段数量:</span>
                      <Badge variant={testResult.sections === 29 ? "default" : "destructive"}>
                        {testResult.sections}
                      </Badge>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-muted-foreground">压缩状态:</span>
                      <Badge variant={testResult.compressed ? "secondary" : "outline"}>
                        {testResult.compressed ? "已压缩" : "未压缩"}
                      </Badge>
                    </div>
                  </div>

                  {/* 错误信息 */}
                  {testResult.errors.length > 0 && (
                    <div className="space-y-2">
                      <h4 className="font-medium text-red-600">错误信息:</h4>
                      <div className="space-y-1">
                        {testResult.errors.map((error, index) => (
                          <div key={index} className="text-sm text-red-600 bg-red-50 p-2 rounded">
                            {error}
                          </div>
                        ))}
                      </div>
                    </div>
                  )}

                  {/* 元数据 */}
                  {testResult.metadata && (
                    <div className="space-y-2">
                      <h4 className="font-medium">元数据:</h4>
                      <pre className="bg-muted p-3 rounded text-xs overflow-auto max-h-32">
                        {JSON.stringify(testResult.metadata, null, 2)}
                      </pre>
                    </div>
                  )}

                  {/* 操作按钮 */}
                  <div className="flex gap-2 pt-4">
                    <Button 
                      variant="outline" 
                      size="sm" 
                      onClick={downloadTestFile}
                      disabled={!selectedFile}
                    >
                      <Download className="mr-2 h-4 w-4" />
                      下载文件
                    </Button>
                    <Button 
                      variant="outline" 
                      size="sm"
                      onClick={() => window.open('/xkt-viewer', '_blank')}
                    >
                      <Eye className="mr-2 h-4 w-4" />
                      打开查看器
                    </Button>
                  </div>
                </div>
              )}
            </CardContent>
          </Card>
        </div>

        {/* 快速测试区域 */}
        <Card className="mt-6">
          <CardHeader>
            <CardTitle>快速测试</CardTitle>
            <CardDescription>
              使用预生成的测试文件进行快速验证
            </CardDescription>
          </CardHeader>
          <CardContent>
            <div className="grid gap-4 md:grid-cols-3">
              <div className="p-4 border rounded-lg">
                <h3 className="font-semibold mb-2">✅ 标准立方体</h3>
                <p className="text-sm text-muted-foreground mb-3">
                  符合 XKT v10 标准的简单立方体模型
                </p>
                <Button 
                  size="sm" 
                  variant="outline"
                  onClick={() => {
                    // 这里可以添加下载标准测试文件的逻辑
                    toast.info("标准测试文件: output/simple_cube_v10.xkt")
                  }}
                >
                  下载测试
                </Button>
              </div>
              <div className="p-4 border rounded-lg">
                <h3 className="font-semibold mb-2">⚠️ 错误版本</h3>
                <p className="text-sm text-muted-foreground mb-3">
                  版本号错误的 XKT 文件，用于测试验证功能
                </p>
                <Button 
                  size="sm" 
                  variant="outline"
                  onClick={() => {
                    toast.info("错误版本文件: output/cube_v10_standard.xkt")
                  }}
                >
                  下载测试
                </Button>
              </div>
              <div className="p-4 border rounded-lg">
                <h3 className="font-semibold mb-2">📊 复杂模型</h3>
                <p className="text-sm text-muted-foreground mb-3">
                  包含多个几何体和实体的复杂模型
                </p>
                <Button 
                  size="sm" 
                  variant="outline"
                  onClick={() => {
                    toast.info("复杂模型文件: output/test_improved.xkt")
                  }}
                >
                  下载测试
                </Button>
              </div>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}


