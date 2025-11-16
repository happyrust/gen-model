"use client"

import { Sidebar } from "@/components/sidebar"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"

export default function ConfigPage() {
  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      <main className="ml-64 p-8">
        <div className="max-w-4xl mx-auto">
          <h1 className="text-3xl font-bold tracking-tight mb-6">配置管理</h1>
          <Card>
            <CardHeader>
              <CardTitle>同步配置</CardTitle>
              <CardDescription>
                管理同步系统的配置参数
              </CardDescription>
            </CardHeader>
            <CardContent>
              <p className="text-sm text-muted-foreground">
                配置管理组件将在任务 10 中实现
              </p>
            </CardContent>
          </Card>
        </div>
      </main>
    </div>
  )
}
