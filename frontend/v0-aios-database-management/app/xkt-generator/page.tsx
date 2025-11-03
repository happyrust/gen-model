"use client"

import { XKTGenerator } from "@/components/xkt-generator"
import { Sidebar } from "@/components/sidebar"
import { Badge } from "@/components/ui/badge"
import { Breadcrumb, BreadcrumbItem, BreadcrumbLink, BreadcrumbList, BreadcrumbSeparator } from "@/components/ui/breadcrumb"

export default function XKTGeneratorPage() {
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
                <BreadcrumbLink>XKT生成器</BreadcrumbLink>
              </BreadcrumbItem>
            </BreadcrumbList>
          </Breadcrumb>

          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-3xl font-bold text-foreground mb-2">
                XKT 模型生成工具
              </h1>
              <p className="text-lg text-muted-foreground">
                将数据库模型转换为用于3D可视化的XKT格式文件
              </p>
            </div>
            <Badge className="bg-primary text-primary-foreground px-4 py-2">
              v10 格式支持
            </Badge>
          </div>
        </div>

        {/* XKT生成器组件 */}
        <XKTGenerator />

        {/* 功能说明 */}
        <div className="mt-8 grid gap-4 md:grid-cols-3">
          <div className="p-4 border rounded-lg">
            <h3 className="font-semibold mb-2">🚀 高性能</h3>
            <p className="text-sm text-muted-foreground">
              采用高效的二进制格式，支持大规模3D模型的快速加载和渲染
            </p>
          </div>
          <div className="p-4 border rounded-lg">
            <h3 className="font-semibold mb-2">📦 智能压缩</h3>
            <p className="text-sm text-muted-foreground">
              可选压缩功能，减少75%文件大小，优化网络传输性能
            </p>
          </div>
          <div className="p-4 border rounded-lg">
            <h3 className="font-semibold mb-2">🎯 精确定位</h3>
            <p className="text-sm text-muted-foreground">
              支持通过参考号精确定位特定区域或组件进行导出
            </p>
          </div>
        </div>
      </div>
    </div>
  )
}