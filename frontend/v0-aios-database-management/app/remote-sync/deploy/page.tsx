"use client"

import { useRouter } from "next/navigation"
import { Sidebar } from "@/components/sidebar"
import { DeployWizard } from "@/components/remote-sync/deploy-wizard"

export default function DeployPage() {
  const router = useRouter()

  const handleComplete = (envId: string) => {
    // 跳转到监控页面
    router.push("/remote-sync/monitor")
  }

  const handleCancel = () => {
    // 返回环境列表
    router.push("/remote-sync")
  }

  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      <main className="ml-64 p-8">
        <div className="mb-6">
          <h1 className="text-3xl font-bold tracking-tight">部署向导</h1>
          <p className="text-muted-foreground mt-2">
            通过向导引导完成环境部署配置
          </p>
        </div>
        <DeployWizard onComplete={handleComplete} onCancel={handleCancel} />
      </main>
    </div>
  )
}
