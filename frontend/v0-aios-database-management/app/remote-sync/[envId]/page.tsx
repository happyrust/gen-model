"use client"

import { Sidebar } from "@/components/sidebar"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { useParams } from "next/navigation"

export default function EnvironmentDetailPage() {
  const params = useParams()
  const envId = params.envId as string

  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      <main className="ml-64 p-8">
        <div className="max-w-7xl mx-auto">
          <h1 className="text-3xl font-bold tracking-tight mb-6">
            环境详情: {envId}
          </h1>
          <Card>
            <CardHeader>
              <CardTitle>环境信息</CardTitle>
              <CardDescription>
                查看和管理环境配置
              </CardDescription>
            </CardHeader>
            <CardContent>
              <p className="text-sm text-muted-foreground">
                环境详情组件将在任务 11 中实现
              </p>
            </CardContent>
          </Card>
        </div>
      </main>
    </div>
  )
}
