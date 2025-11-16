"use client"

import { useState } from "react"
import { Button } from "@/components/ui/button"
import { Progress } from "@/components/ui/progress"
import { ChevronLeft, CheckCircle2, Loader2 } from "lucide-react"
import { useCreateEnvironment, useActivateEnvironment } from "@/hooks/use-environments"
import { useCreateSite } from "@/hooks/use-sites"
import { toast } from "sonner"
import type { Environment, Site } from "@/types/remote-sync"

interface StepActivationProps {
  environment: Partial<Environment>
  sites: Partial<Site>[]
  onComplete: (envId: string) => void
  onPrevious: () => void
  onCancel: () => void
}

export function StepActivation({
  environment,
  sites,
  onComplete,
  onPrevious,
  onCancel,
}: StepActivationProps) {
  const [activating, setActivating] = useState(false)
  const [progress, setProgress] = useState(0)
  const [currentStep, setCurrentStep] = useState("")
  const [error, setError] = useState<string | null>(null)

  const createEnvironment = useCreateEnvironment()
  const createSite = useCreateSite()
  const activateEnvironment = useActivateEnvironment()

  const handleActivate = async () => {
    setActivating(true)
    setError(null)
    setProgress(0)

    try {
      // 步骤 1: 创建环境
      setCurrentStep("创建环境...")
      setProgress(20)
      
      const envId = await createEnvironment.mutateAsync(environment)
      
      setProgress(40)

      // 步骤 2: 创建站点
      if (sites.length > 0) {
        setCurrentStep(`创建站点 (0/${sites.length})...`)
        
        for (let i = 0; i < sites.length; i++) {
          await createSite.mutateAsync({
            envId,
            data: sites[i],
          })
          setCurrentStep(`创建站点 (${i + 1}/${sites.length})...`)
          setProgress(40 + (30 * (i + 1)) / sites.length)
        }
      }

      setProgress(70)

      // 步骤 3: 激活环境
      setCurrentStep("激活环境...")
      await activateEnvironment.mutateAsync(envId)
      
      setProgress(100)
      setCurrentStep("完成！")

      toast.success("环境部署成功！")
      
      // 延迟一下再跳转，让用户看到完成状态
      setTimeout(() => {
        onComplete(envId)
      }, 1000)
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : "部署失败"
      setError(errorMessage)
      toast.error(errorMessage)
      setActivating(false)
    }
  }

  return (
    <div className="space-y-6">
      {/* 配置摘要 */}
      <div className="space-y-4">
        <h3 className="font-medium">配置摘要</h3>
        
        <div className="space-y-2 p-4 bg-muted rounded-lg">
          <div className="flex items-center justify-between">
            <span className="text-sm text-muted-foreground">环境名称</span>
            <span className="font-medium">{environment.name}</span>
          </div>
          <div className="flex items-center justify-between">
            <span className="text-sm text-muted-foreground">位置</span>
            <span className="font-medium">{environment.location || "未指定"}</span>
          </div>
          <div className="flex items-center justify-between">
            <span className="text-sm text-muted-foreground">MQTT 服务器</span>
            <span className="font-medium">
              {environment.mqttHost}:{environment.mqttPort}
            </span>
          </div>
          {environment.fileServerHost && (
            <div className="flex items-center justify-between">
              <span className="text-sm text-muted-foreground">文件服务器</span>
              <span className="font-medium">{environment.fileServerHost}</span>
            </div>
          )}
          <div className="flex items-center justify-between">
            <span className="text-sm text-muted-foreground">站点数量</span>
            <span className="font-medium">{sites.length} 个</span>
          </div>
          {environment.locationDbs && (
            <div className="flex items-center justify-between">
              <span className="text-sm text-muted-foreground">数据库编号</span>
              <span className="font-medium">{environment.locationDbs}</span>
            </div>
          )}
        </div>
      </div>

      {/* 站点列表 */}
      {sites.length > 0 && (
        <div className="space-y-4">
          <h3 className="font-medium">站点列表</h3>
          <div className="space-y-2">
            {sites.map((site, index) => (
              <div key={index} className="p-3 border rounded-lg">
                <div className="font-medium">{site.name}</div>
                <div className="text-sm text-muted-foreground">
                  {site.httpHost} • {site.dbnums}
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 激活进度 */}
      {activating && (
        <div className="space-y-3">
          <div className="flex items-center justify-between text-sm">
            <span className="text-muted-foreground">{currentStep}</span>
            <span className="font-medium">{Math.round(progress)}%</span>
          </div>
          <Progress value={progress} className="h-2" />
        </div>
      )}

      {/* 错误信息 */}
      {error && (
        <div className="p-4 bg-destructive/10 text-destructive rounded-lg">
          {error}
        </div>
      )}

      {/* 成功提示 */}
      {progress === 100 && !error && (
        <div className="p-4 bg-green-500/10 text-green-600 rounded-lg flex items-center gap-2">
          <CheckCircle2 className="h-5 w-5" />
          <span>环境部署成功！正在跳转...</span>
        </div>
      )}

      {/* 注意事项 */}
      {!activating && (
        <div className="p-4 bg-blue-500/10 text-blue-600 rounded-lg">
          <p className="text-sm">
            <strong>注意：</strong>
            激活环境将会：
          </p>
          <ul className="text-sm mt-2 space-y-1 list-disc list-inside">
            <li>将配置写入 DbOption.toml 文件</li>
            <li>启动 MQTT 订阅服务</li>
            <li>启动文件监控服务</li>
            <li>开始自动同步数据</li>
          </ul>
        </div>
      )}

      {/* 导航按钮 */}
      <div className="flex items-center justify-between">
        <Button variant="outline" onClick={onPrevious} disabled={activating}>
          <ChevronLeft className="h-4 w-4 mr-2" />
          上一步
        </Button>
        <div className="flex items-center gap-2">
          <Button variant="outline" onClick={onCancel} disabled={activating}>
            取消
          </Button>
          <Button onClick={handleActivate} disabled={activating}>
            {activating ? (
              <>
                <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                激活中...
              </>
            ) : (
              "激活环境"
            )}
          </Button>
        </div>
      </div>
    </div>
  )
}
