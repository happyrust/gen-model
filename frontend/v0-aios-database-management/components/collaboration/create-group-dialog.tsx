"use client"

import { useState } from "react"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { AlertCircle, Loader2 } from "lucide-react"
import type { CollaborationGroup } from "@/types/collaboration"
import { createRemoteSyncEnv, envToGroup } from "@/lib/api/collaboration-adapter"

interface CreateGroupDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSuccess: (group: CollaborationGroup) => void
}

export function CreateGroupDialog({ open, onOpenChange, onSuccess }: CreateGroupDialogProps) {
  const [step, setStep] = useState(1)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Form data
  const [name, setName] = useState("")
  const [description, setDescription] = useState("")
  const [location, setLocation] = useState("")
  const [mqttHost, setMqttHost] = useState("")
  const [mqttPort, setMqttPort] = useState(1883)
  const [mqttUser, setMqttUser] = useState("")
  const [mqttPassword, setMqttPassword] = useState("")
  const [fileServerHost, setFileServerHost] = useState("")

  const resetForm = () => {
    setStep(1)
    setName("")
    setDescription("")
    setLocation("")
    setMqttHost("")
    setMqttPort(1883)
    setMqttUser("")
    setMqttPassword("")
    setFileServerHost("")
    setError(null)
  }

  const handleNext = () => {
    if (step === 1) {
      if (!name.trim()) {
        setError("请输入协同组名称")
        return
      }
      if (!mqttHost.trim()) {
        setError("请输入 MQTT 服务器地址")
        return
      }
      setError(null)
      setStep(2)
    } else if (step === 2) {
      setError(null)
      setStep(2)
    }
  }

  const handlePrevious = () => {
    setError(null)
    setStep(step - 1)
  }

  const handleSubmit = async () => {
    if (!name.trim()) {
      setError("请输入协同组名称")
      return
    }
    if (!mqttHost.trim()) {
      setError("请输入 MQTT 服务器地址")
      return
    }

    setLoading(true)
    setError(null)

    try {
      const env = await createRemoteSyncEnv({
        name,
        mqtt_host: mqttHost,
        mqtt_port: mqttPort,
        mqtt_user: mqttUser || undefined,
        mqtt_password: mqttPassword || undefined,
        file_server_host: fileServerHost || undefined,
        location: location || undefined,
        reconnect_initial_ms: 1000,
        reconnect_max_ms: 60000,
      })

      const group = envToGroup(env)
      onSuccess(group)
      resetForm()
      onOpenChange(false)
    } catch (err) {
      setError(err instanceof Error ? err.message : "创建协同组失败")
    } finally {
      setLoading(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[600px]">
        <DialogHeader>
          <DialogTitle>创建协同组</DialogTitle>
          <DialogDescription>步骤 {step} / 2: {step === 1 ? "基本信息" : "站点配置"}</DialogDescription>
        </DialogHeader>

        <div className="py-4">
          {/* Step 1: 基本信息与 MQTT 配置 */}
          {step === 1 && (
            <div className="space-y-4">
              <div className="space-y-2">
                <Label htmlFor="name">环境名称 *</Label>
                <Input
                  id="name"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="例如：北京-上海协同环境"
                />
              </div>

              <div className="space-y-2">
                <Label htmlFor="location">位置描述</Label>
                <Input
                  id="location"
                  value={location}
                  onChange={(e) => setLocation(e.target.value)}
                  placeholder="例如：北京数据中心"
                />
              </div>

              <div className="border-t pt-4 mt-4">
                <h4 className="text-sm font-medium mb-3">MQTT 服务器配置</h4>

                <div className="space-y-3">
                  <div className="space-y-2">
                    <Label htmlFor="mqtt-host">MQTT 服务器地址 *</Label>
                    <Input
                      id="mqtt-host"
                      value={mqttHost}
                      onChange={(e) => setMqttHost(e.target.value)}
                      placeholder="例如：mqtt.example.com"
                    />
                  </div>

                  <div className="space-y-2">
                    <Label htmlFor="mqtt-port">MQTT 端口</Label>
                    <Input
                      id="mqtt-port"
                      type="number"
                      value={mqttPort}
                      onChange={(e) => setMqttPort(Number(e.target.value))}
                      placeholder="1883"
                    />
                  </div>

                  <div className="space-y-2">
                    <Label htmlFor="mqtt-user">MQTT 用户名（可选）</Label>
                    <Input
                      id="mqtt-user"
                      value={mqttUser}
                      onChange={(e) => setMqttUser(e.target.value)}
                      placeholder="MQTT 用户名"
                    />
                  </div>

                  <div className="space-y-2">
                    <Label htmlFor="mqtt-password">MQTT 密码（可选）</Label>
                    <Input
                      id="mqtt-password"
                      type="password"
                      value={mqttPassword}
                      onChange={(e) => setMqttPassword(e.target.value)}
                      placeholder="MQTT 密码"
                    />
                  </div>

                  <div className="space-y-2">
                    <Label htmlFor="file-server">文件服务器地址（可选）</Label>
                    <Input
                      id="file-server"
                      value={fileServerHost}
                      onChange={(e) => setFileServerHost(e.target.value)}
                      placeholder="例如：http://files.example.com"
                    />
                  </div>
                </div>
              </div>
            </div>
          )}

          {/* Step 2: 站点配置 */}
          {step === 2 && (
            <div className="space-y-4">
              <p className="text-sm text-muted-foreground">
                环境创建后，可以在详情页面添加和管理站点。
              </p>
            </div>
          )}
        </div>

        {/* Error Message */}
        {error && (
          <div className="flex items-center gap-2 p-3 text-sm text-destructive bg-destructive/10 rounded-md">
            <AlertCircle className="h-4 w-4 flex-shrink-0" />
            <span>{error}</span>
          </div>
        )}

        <DialogFooter>
          <div className="flex justify-between w-full">
            <div>
              {step > 1 && (
                <Button variant="outline" onClick={handlePrevious} disabled={loading}>
                  上一步
                </Button>
              )}
            </div>
            <div className="flex gap-2">
              <Button variant="outline" onClick={() => onOpenChange(false)} disabled={loading}>
                取消
              </Button>
              {step < 2 ? (
                <Button onClick={handleNext} disabled={loading}>
                  下一步
                </Button>
              ) : (
                <Button onClick={handleSubmit} disabled={loading}>
                  {loading ? (
                    <>
                      <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                      创建中...
                    </>
                  ) : (
                    "创建协同环境"
                  )}
                </Button>
              )}
            </div>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}