"use client"

import { useState, useEffect } from "react"
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
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible"
import { AlertCircle, Loader2, ChevronDown, ChevronRight } from "lucide-react"
import type { CollaborationGroup } from "@/types/collaboration"
import { createRemoteSyncEnv, createRemoteSyncSite, envToGroup } from "@/lib/api/collaboration-adapter"
import { getPublicApiBaseUrl } from "@/lib/env"
import { buildApiUrl } from "@/lib/api"
import { Checkbox } from "@/components/ui/checkbox"
import { toast } from "sonner"

interface CreateGroupDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSuccess: (group: CollaborationGroup) => void
}

export function CreateGroupDialog({ open, onOpenChange, onSuccess }: CreateGroupDialogProps) {
  const [step, setStep] = useState(1)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [mqttConfigOpen, setMqttConfigOpen] = useState(true)
  const [advancedNetworkOpen, setAdvancedNetworkOpen] = useState(false)
  const [configLoading, setConfigLoading] = useState(false)
  const apiBaseUrl = getPublicApiBaseUrl()

  // Form data
  const [name, setName] = useState("")
  const [description, setDescription] = useState("")
  const [location, setLocation] = useState("")
  const [mqttHost, setMqttHost] = useState("")
  const [mqttPort, setMqttPort] = useState(1883)
  const [mqttUser, setMqttUser] = useState("")
  const [mqttPassword, setMqttPassword] = useState("")
  const [fileServerHost, setFileServerHost] = useState("")
  const [reconnectInitialMs, setReconnectInitialMs] = useState(1000)
  const [reconnectMaxMs, setReconnectMaxMs] = useState(60000)

  const [siteName, setSiteName] = useState("")
  const [siteDescription, setSiteDescription] = useState("")
  const [siteLocationDetail, setSiteLocationDetail] = useState("")
  const [siteHost, setSiteHost] = useState("")
  const [siteDbNums, setSiteDbNums] = useState("")
  const [siteIsLocal, setSiteIsLocal] = useState(false)

  // 加载默认配置
  useEffect(() => {
    if (!open) return

    const loadDefaultConfig = async () => {
      setConfigLoading(true)
      try {
        // 尝试从 API 获取运行时配置
        const response = await fetch(buildApiUrl("/api/remote-sync/runtime/config"), {
          method: "GET",
          headers: { Accept: "application/json" },
        })

        if (response.ok) {
          const data = await response.json()
          if (data.status === "success" && data.config) {
            const config = data.config
            // 设置 MQTT 主机地址（优先使用配置，否则使用本机地址）
            if (config.mqtt_host) {
              setMqttHost(config.mqtt_host)
            } else if (typeof window !== "undefined") {
              setMqttHost(window.location.hostname)
            }
            // 设置 MQTT 端口
            if (config.mqtt_port) {
              setMqttPort(config.mqtt_port)
            }
            // 设置文件服务器地址
            if (config.file_server_host) {
              setFileServerHost(config.file_server_host)
            }
            // 设置位置（只在没有手动设置时使用配置值）
            if (config.location) {
              setLocation((prev) => prev || config.location)
            }
            if (config.reconnect_initial_ms) {
              setReconnectInitialMs(config.reconnect_initial_ms)
            }
            if (config.reconnect_max_ms) {
              setReconnectMaxMs(config.reconnect_max_ms)
            }
          }
        } else {
          // API 失败时使用本机地址作为默认值
          if (typeof window !== "undefined") {
            setMqttHost((prev) => prev || window.location.hostname)
          }
        }
      } catch (err) {
        // 出错时使用本机地址作为默认值
        if (typeof window !== "undefined") {
          setMqttHost((prev) => prev || window.location.hostname)
        }
      } finally {
        setConfigLoading(false)
      }
    }

    loadDefaultConfig()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

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
    setReconnectInitialMs(1000)
    setReconnectMaxMs(60000)
    setSiteName("")
    setSiteDescription("")
    setSiteLocationDetail("")
    setSiteHost("")
    setSiteDbNums("")
    setSiteIsLocal(false)
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
    const hasSiteInput =
      siteName.trim() ||
      siteDescription.trim() ||
      siteLocationDetail.trim() ||
      siteHost.trim() ||
      siteDbNums.trim()
    if (hasSiteInput && !siteName.trim()) {
      setError("请输入首个站点名称，或清空站点表单。")
      return
    }
    if (!apiBaseUrl) {
      setError("未配置 NEXT_PUBLIC_API_BASE_URL，无法创建协同组。请在 .env.local 中设置后重试。")
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
        reconnect_initial_ms: reconnectInitialMs || undefined,
        reconnect_max_ms: reconnectMaxMs || undefined,
      })

      if (hasSiteInput) {
        try {
          await createRemoteSyncSite(env.id, {
            site_name: siteName.trim(),
            site_description: siteDescription || undefined,
            site_host: siteHost || undefined,
            site_location: siteLocationDetail || undefined,
            site_location_dbs: siteDbNums || undefined,
            is_local: siteIsLocal,
          })
          toast.success("已创建首个站点")
        } catch (siteErr) {
          toast.error(
            siteErr instanceof Error
              ? `环境创建成功，但站点创建失败：${siteErr.message}`
              : "环境创建成功，但站点创建失败",
          )
        }
      }

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

              {configLoading && (
                <div className="flex items-center gap-2 text-xs text-muted-foreground">
                  <Loader2 className="h-3 w-3 animate-spin" />
                  正在读取运行时默认配置...
                </div>
              )}

              <Collapsible open={mqttConfigOpen} onOpenChange={setMqttConfigOpen} className="border-t pt-4 mt-4">
                <CollapsibleTrigger className="flex w-full items-center justify-between rounded-lg px-2 py-2 text-left text-sm font-medium hover:bg-accent">
                  <span>MQTT 服务器配置</span>
                  {mqttConfigOpen ? (
                    <ChevronDown className="h-4 w-4" />
                  ) : (
                    <ChevronRight className="h-4 w-4" />
                  )}
                </CollapsibleTrigger>
                <CollapsibleContent className="space-y-3 pt-3">
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
                      placeholder="例如：http://files.example.com 或 file:///data/dpcsync"
                    />
                  </div>
                </CollapsibleContent>
              </Collapsible>

              <Collapsible
                open={advancedNetworkOpen}
                onOpenChange={setAdvancedNetworkOpen}
                className="border-t pt-4"
              >
                <CollapsibleTrigger className="flex w-full items-center justify-between rounded-lg px-2 py-2 text-left text-sm font-medium hover:bg-accent">
                  <span>高级容错与网络设置</span>
                  {advancedNetworkOpen ? (
                    <ChevronDown className="h-4 w-4" />
                  ) : (
                    <ChevronRight className="h-4 w-4" />
                  )}
                </CollapsibleTrigger>
                <CollapsibleContent className="grid gap-3 pt-3 md:grid-cols-2">
                  <div className="space-y-2">
                    <Label htmlFor="reconnect-initial">重连初始间隔 (ms)</Label>
                    <Input
                      id="reconnect-initial"
                      type="number"
                      value={reconnectInitialMs}
                      min={100}
                      onChange={(e) => setReconnectInitialMs(Number(e.target.value))}
                      placeholder="1000"
                    />
                  </div>
                  <div className="space-y-2">
                    <Label htmlFor="reconnect-max">重连最大间隔 (ms)</Label>
                    <Input
                      id="reconnect-max"
                      type="number"
                      value={reconnectMaxMs}
                      min={1000}
                      onChange={(e) => setReconnectMaxMs(Number(e.target.value))}
                      placeholder="60000"
                    />
                  </div>
                </CollapsibleContent>
              </Collapsible>
            </div>
          )}

          {/* Step 2: 站点配置 */}
          {step === 2 && (
            <div className="space-y-4">
              <p className="text-sm text-muted-foreground">
                可选：立即登记首个同步站点，也可以留空并在协同详情页补充。
              </p>

              <div className="grid gap-3">
                <div className="space-y-2">
                  <Label htmlFor="site-name">站点名称 {siteName ? "*" : "(可选)"}</Label>
                  <Input
                    id="site-name"
                    value={siteName}
                    onChange={(e) => setSiteName(e.target.value)}
                    placeholder="例如：上海外场站"
                  />
                </div>

                <div className="space-y-2">
                  <Label htmlFor="site-location">站点位置 / 备注</Label>
                  <Input
                    id="site-location"
                    value={siteLocationDetail}
                    onChange={(e) => setSiteLocationDetail(e.target.value)}
                    placeholder="例如：上海机房 B1"
                  />
                </div>

                <div className="space-y-2">
                  <Label htmlFor="site-host">文件/HTTP 地址</Label>
                  <Input
                    id="site-host"
                    value={siteHost}
                    onChange={(e) => setSiteHost(e.target.value)}
                    placeholder="例如：http://192.168.1.10:8080 或 file:///data/site-a"
                  />
                </div>

                <div className="space-y-2">
                  <Label htmlFor="site-dbnums">数据库编号（逗号分隔，可选）</Label>
                  <Input
                    id="site-dbnums"
                    value={siteDbNums}
                    onChange={(e) => setSiteDbNums(e.target.value)}
                    placeholder="例如：DESI,GLB"
                  />
                </div>

                <div className="space-y-2">
                  <Label htmlFor="site-description">站点说明（可选）</Label>
                  <Input
                    id="site-description"
                    value={siteDescription}
                    onChange={(e) => setSiteDescription(e.target.value)}
                    placeholder="例如：负责结构模型增量"
                  />
                </div>

                <div className="flex items-center gap-2">
                  <Checkbox
                    id="site-local"
                    checked={siteIsLocal}
                    onCheckedChange={(checked) => setSiteIsLocal(Boolean(checked))}
                  />
                  <Label htmlFor="site-local" className="text-sm text-muted-foreground">
                    该站点位于当前节点（本地文件可直接写入）
                  </Label>
                </div>
              </div>
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
