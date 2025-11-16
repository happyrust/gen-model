"use client"

import { Sidebar } from "@/components/sidebar"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"

export default function FlowPage() {
  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      <main className="ml-64 p-8">
        <div className="max-w-7xl mx-auto">
          <h1 className="text-3xl font-bold tracking-tight mb-6">数据流向可视化</h1>
          <Card>
            <CardHeader>
              <CardTitle>流向图</CardTitle>
              <CardDescription>
                可视化展示数据在各站点间的流向
              </CardDescription>
            </CardHeader>
            <CardContent>
              <p className="text-sm text-muted-foreground">
                流向可视化组件将在任务 4 中实现
              </p>
            </CardContent>
          </Card>
        </div>
      </main>
    </div>
  )
}
