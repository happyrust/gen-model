"use client"

import { Sidebar } from "@/components/sidebar"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"

export default function MetricsPage() {
  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      <main className="ml-64 p-8">
        <div className="max-w-7xl mx-auto">
          <h1 className="text-3xl font-bold tracking-tight mb-6">性能监控</h1>
          <Card>
            <CardHeader>
              <CardTitle>性能指标</CardTitle>
              <CardDescription>
                展示历史性能趋势和实时指标
              </CardDescription>
            </CardHeader>
            <CardContent>
              <p className="text-sm text-muted-foreground">
                性能监控组件将在任务 6 中实现
              </p>
            </CardContent>
          </Card>
        </div>
      </main>
    </div>
  )
}
