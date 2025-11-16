"use client"

import { Sidebar } from "@/components/sidebar"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { useParams } from "next/navigation"

export default function SiteDetailPage() {
  const params = useParams()
  const envId = params.envId as string
  const siteId = params.siteId as string

  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      <main className="ml-64 p-8">
        <div className="max-w-7xl mx-auto">
          <h1 className="text-3xl font-bold tracking-tight mb-6">
            站点详情: {siteId}
          </h1>
          <Card>
            <CardHeader>
              <CardTitle>站点元数据</CardTitle>
              <CardDescription>
                浏览和管理站点的同步文件元数据
              </CardDescription>
            </CardHeader>
            <CardContent>
              <p className="text-sm text-muted-foreground">
                站点详情组件将在任务 7 中实现
              </p>
            </CardContent>
          </Card>
        </div>
      </main>
    </div>
  )
}
