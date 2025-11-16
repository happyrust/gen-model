"use client"

import { Sidebar } from "@/components/sidebar"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"

export default function LogsPage() {
  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      <main className="ml-64 p-8">
        <div className="max-w-7xl mx-auto">
          <h1 className="text-3xl font-bold tracking-tight mb-6">日志查询</h1>
          <Card>
            <CardHeader>
              <CardTitle>同步日志</CardTitle>
              <CardDescription>
                查询和分析同步日志
              </CardDescription>
            </CardHeader>
            <CardContent>
              <p className="text-sm text-muted-foreground">
                日志查询组件将在任务 5 中实现
              </p>
            </CardContent>
          </Card>
        </div>
      </main>
    </div>
  )
}
