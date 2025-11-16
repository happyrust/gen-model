"use client"

import { useState } from "react"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { ChevronLeft, ChevronRight, CheckCircle2, XCircle, Loader2 } from "lucide-react"
import { testConnection } from "@/lib/api/remote-sync"
import type { Environment, Site } from "@/types/remote-sync"

interface StepConnectionTestProps {
  environment: Partial<Environment>
  sites: Partial<Site>[]
  testResults: {
    mqttConnected: boolean
    httpReachable: boolean
    latency: number
  }
  onTestComplete: (results: Partial<StepConnectionTestProps["testResults"]>) => void
  onNext: () => void
  onPrevious: () => void
}

export function StepConnectionTest({
  environment,
  sites,
  testResults,
  onTestComplete,
  onNext,
  onPrevious,
}: StepConnectionTestProps) {
  const [testing, setTesting] = useState(false)
  const [tested, setTested] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const handleTest = async () => {
    setTesting(true)
    setError(null)

    try {
      // 测试 MQTT 连接
      if (environment.mqttHost && environment.mqttPort) {
        const result = await testConnection({
          mqtt_host: environment.mqttHost,
          mqtt_port: environment.mqttPort,
        })

        onTestComplete({
          mqttConnected: result.mqtt_connected || false,
          httpReachable: result.http_reachable || false,
          latency: result.latency || 0,
        })
      }

      setTested(true)
    } catch (err) {
      setError(err instanceof Error ? err.message : "连接测试失败")
      onTestComplete({
        mqttConnected: false,
        httpReachable: false,
        latency: 0,
      })
    } finally {
      setTesting(false)
    }
  }

  return (
    <div className="space-y-6">
      {/* 测试说明 */}
      <div className="p-4 bg-muted rounded-lg">
        <p className="text-sm text-muted-foreground">
          在激活环境之前，建议测试 MQTT 和文件服务器的连接状态，确保配置正确。
        </p>
      </div>

      {/* 配置预览 */}
      <div className="space-y-4">
        <h3 className="font-medium">配置预览</h3>
        
        <div className="space-y-3">
          <div className="flex items-center justify-between p-3 border rounded-lg">
            <div>
              <div className="font-medium">环境名称</div>
              <div className="text-sm text-muted-foreground">{environment.name}</div>
            </div>
          </div>

          <div className="flex items-center justify-between p-3 border rounded-lg">
            <div>
              <div className="font-medium">MQTT 服务器</div>
              <div className="text-sm text-muted-foreground">
                {environment.mqttHost}:{environment.mqttPort}
              </div>
            </div>
            {tested && (
              <Badge variant={testResults.mqttConnected ? "default" : "destructive"}>
                {testResults.mqttConnected ? (
                  <>
                    <CheckCircle2 className="h-3 w-3 mr-1" />
                    已连接
                  </>
                ) : (
                  <>
                    <XCircle className="h-3 w-3 mr-1" />
                    连接失败
                  </>
                )}
              </Badge>
            )}
          </div>

          {environment.fileServerHost && (
            <div className="flex items-center justify-between p-3 border rounded-lg">
              <div>
                <div className="font-medium">文件服务器</div>
                <div className="text-sm text-muted-foreground">
                  {environment.fileServerHost}
                </div>
              </div>
              {tested && (
                <Badge variant={testResults.httpReachable ? "default" : "destructive"}>
                  {testResults.httpReachable ? (
                    <>
                      <CheckCircle2 className="h-3 w-3 mr-1" />
                      可访问
                    </>
                  ) : (
                    <>
                      <XCircle className="h-3 w-3 mr-1" />
                      不可访问
                    </>
                  )}
                </Badge>
              )}
            </div>
          )}

          <div className="flex items-center justify-between p-3 border rounded-lg">
            <div>
              <div className="font-medium">站点数量</div>
              <div className="text-sm text-muted-foreground">{sites.length} 个站点</div>
            </div>
          </div>

          {tested && testResults.latency > 0 && (
            <div className="flex items-center justify-between p-3 border rounded-lg">
              <div>
                <div className="font-medium">网络延迟</div>
                <div className="text-sm text-muted-foreground">
                  {testResults.latency} ms
                </div>
              </div>
            </div>
          )}
        </div>
      </div>

      {/* 错误信息 */}
      {error && (
        <div className="p-4 bg-destructive/10 text-destructive rounded-lg">
          {error}
        </div>
      )}

      {/* 测试按钮 */}
      <div className="flex justify-center">
        <Button
          onClick={handleTest}
          disabled={testing || !environment.mqttHost}
          size="lg"
        >
          {testing ? (
            <>
              <Loader2 className="h-4 w-4 mr-2 animate-spin" />
              测试中...
            </>
          ) : (
            "测试连接"
          )}
        </Button>
      </div>

      {/* 导航按钮 */}
      <div className="flex items-center justify-between">
        <Button variant="outline" onClick={onPrevious}>
          <ChevronLeft className="h-4 w-4 mr-2" />
          上一步
        </Button>
        <Button onClick={onNext} disabled={!tested}>
          下一步
          <ChevronRight className="h-4 w-4 ml-2" />
        </Button>
      </div>
    </div>
  )
}
