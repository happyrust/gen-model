"use client"

import { useState } from "react"
import { SiteCard, type Site } from "@/components/deployment-sites/site-card"

// 模拟数据
const mockSites: Site[] = [
  {
    id: "1",
    name: "happyrust",
    status: "configuring",
    environment: "dev",
    owner: "张三",
    createdAt: new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString(),
    updatedAt: new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString(),
    description: "开发环境测试站点",
    url: "http://localhost:3000",
    dbStatus: "running",
    parsingStatus: "completed",
    modelGenerationStatus: "generating"
  },
  {
    id: "2", 
    name: "3drepobouncer",
    status: "configuring",
    environment: "dev",
    owner: "李四",
    createdAt: new Date(Date.now() - 27 * 24 * 60 * 60 * 1000).toISOString(),
    updatedAt: new Date(Date.now() - 27 * 24 * 60 * 60 * 1000).toISOString(),
    description: "3D仓库管理站点",
    url: "http://localhost:3001",
    dbStatus: "stopped",
    parsingStatus: "parsing",
    modelGenerationStatus: "unknown"
  },
  {
    id: "3",
    name: "向导部署站点-YCYK-E3D",
    status: "configuring", 
    environment: "dev",
    owner: "王五",
    createdAt: new Date(Date.now() - 30 * 24 * 60 * 60 * 1000).toISOString(),
    updatedAt: new Date(Date.now() - 30 * 24 * 60 * 60 * 1000).toISOString(),
    description: "E3D向导部署站点",
    url: "http://localhost:3002",
    dbStatus: "starting",
    parsingStatus: "failed",
    modelGenerationStatus: "completed"
  }
]

export default function SiteCardDemo() {
  const [sites] = useState<Site[]>(mockSites)

  const handleSiteView = (site: Site) => {
    console.log("查看站点:", site.name)
  }

  const handleSiteStart = (site: Site) => {
    console.log("启动站点:", site.name)
  }

  const handleSitePause = (site: Site) => {
    console.log("暂停站点:", site.name)
  }

  const handleSiteConfigure = (site: Site) => {
    console.log("配置站点:", site.name)
  }

  const handleSiteDelete = (site: Site) => {
    console.log("删除站点:", site.name)
  }

  return (
    <div className="min-h-screen bg-background p-8">
      <div className="max-w-6xl mx-auto">
        <h1 className="text-3xl font-bold mb-8">部署站点管理 - 弹窗详情演示</h1>
        
        <div className="mb-6">
          <h2 className="text-xl font-semibold mb-4">功能说明</h2>
          <div className="bg-blue-50 border border-blue-200 rounded-lg p-4">
            <ul className="space-y-2 text-sm">
              <li>• 点击"查看详情"按钮会打开弹窗，而不是跳转到新页面</li>
              <li>• 每个站点卡片现在显示数据库状态、解析状态和模型生成状态</li>
              <li>• 弹窗中显示完整的站点配置信息，包括数据库控制功能</li>
              <li>• 状态图标：绿色✓表示完成/运行中，蓝色时钟表示进行中，红色✗表示失败/停止</li>
            </ul>
          </div>
        </div>

        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {sites.map((site) => (
            <SiteCard
              key={site.id}
              site={site}
              onView={handleSiteView}
              onStart={handleSiteStart}
              onPause={handleSitePause}
              onConfigure={handleSiteConfigure}
              onDelete={handleSiteDelete}
            />
          ))}
        </div>
      </div>
    </div>
  )
}






