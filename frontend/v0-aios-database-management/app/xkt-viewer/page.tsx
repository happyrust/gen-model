"use client"

import { XKTViewer } from "@/components/xkt-viewer"
import { Sidebar } from "@/components/sidebar"
import { Badge } from "@/components/ui/badge"
import { Breadcrumb, BreadcrumbItem, BreadcrumbLink, BreadcrumbList, BreadcrumbSeparator } from "@/components/ui/breadcrumb"

export default function XKTViewerPage() {
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
                <BreadcrumbLink>XKT查看器</BreadcrumbLink>
              </BreadcrumbItem>
            </BreadcrumbList>
          </Breadcrumb>

          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-3xl font-bold text-foreground mb-2">
                XKT 文件测试查看器
              </h1>
              <p className="text-lg text-muted-foreground">
                上传和预览 XKT 格式的 3D 模型文件
              </p>
            </div>
            <div className="flex gap-2">
              <Badge className="bg-primary text-primary-foreground px-4 py-2">
                v10 格式支持
              </Badge>
              <Badge variant="outline" className="px-4 py-2">
                3D 预览
              </Badge>
            </div>
          </div>
        </div>

        {/* XKT查看器组件 */}
        <XKTViewer />

        {/* 功能说明 */}
        <div className="mt-8 grid gap-4 md:grid-cols-3">
          <div className="p-4 border rounded-lg">
            <h3 className="font-semibold mb-2">📁 文件上传</h3>
            <p className="text-sm text-muted-foreground">
              支持拖拽上传 XKT 文件，自动验证文件格式和完整性
            </p>
          </div>
          <div className="p-4 border rounded-lg">
            <h3 className="font-semibold mb-2">🔍 实时预览</h3>
            <p className="text-sm text-muted-foreground">
              基于 xeokit 的 3D 查看器，支持旋转、缩放、平移操作
            </p>
          </div>
          <div className="p-4 border rounded-lg">
            <h3 className="font-semibold mb-2">📊 文件分析</h3>
            <p className="text-sm text-muted-foreground">
              显示文件详细信息，包括几何数据、实体数量、压缩状态等
            </p>
          </div>
        </div>
      </div>
    </div>
  )
}


