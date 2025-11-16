"use client"

import { useState } from "react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import { ChevronRight } from "lucide-react"
import type { Environment } from "@/types/remote-sync"

interface StepBasicInfoProps {
  data: Partial<Environment>
  onChange: (data: Partial<Environment>) => void
  onNext: () => void
}

export function StepBasicInfo({ data, onChange, onNext }: StepBasicInfoProps) {
  const [errors, setErrors] = useState<Record<string, string>>({})

  const validate = () => {
    const newErrors: Record<string, string> = {}

    if (!data.name || data.name.trim().length === 0) {
      newErrors.name = "环境名称不能为空"
    }

    if (data.mqttHost && !/^[a-zA-Z0-9.-]+$/.test(data.mqttHost)) {
      newErrors.mqttHost = "MQTT 主机地址格式不正确"
    }

    if (data.mqttPort && (data.mqttPort < 1 || data.mqttPort > 65535)) {
      newErrors.mqttPort = "端口号必须在 1-65535 之间"
    }

    if (data.fileServerHost && !/^https?:\/\/.+/.test(data.fileServerHost)) {
      newErrors.fileServerHost = "文件服务器地址格式不正确（需要 http:// 或 https://）"
    }

    setErrors(newErrors)
    return Object.keys(newErrors).length === 0
  }

  const handleNext = () => {
    if (validate()) {
      onNext()
    }
  }

  return (
    <div className="space-y-6">
      {/* 环境名称 */}
      <div className="space-y-2">
        <Label htmlFor="name">
          环境名称 <span className="text-destructive">*</span>
        </Label>
        <Input
          id="name"
          placeholder="例如：北京数据中心"
          value={data.name || ""}
          onChange={(e) => onChange({ name: e.target.value })}
          className={errors.name ? "border-destructive" : ""}
        />
        {errors.name && <p className="text-sm text-destructive">{errors.name}</p>}
      </div>

      {/* 位置描述 */}
      <div className="space-y-2">
        <Label htmlFor="location">位置描述</Label>
        <Input
          id="location"
          placeholder="例如：北京"
          value={data.location || ""}
          onChange={(e) => onChange({ location: e.target.value })}
        />
      </div>

      {/* MQTT 配置 */}
      <div className="space-y-4">
        <h3 className="text-sm font-medium">MQTT 配置</h3>
        
        <div className="grid grid-cols-2 gap-4">
          <div className="space-y-2">
            <Label htmlFor="mqttHost">MQTT 主机地址</Label>
            <Input
              id="mqttHost"
              placeholder="例如：mqtt.example.com"
              value={data.mqttHost || ""}
              onChange={(e) => onChange({ mqttHost: e.target.value })}
              className={errors.mqttHost ? "border-destructive" : ""}
            />
            {errors.mqttHost && <p className="text-sm text-destructive">{errors.mqttHost}</p>}
          </div>

          <div className="space-y-2">
            <Label htmlFor="mqttPort">MQTT 端口</Label>
            <Input
              id="mqttPort"
              type="number"
              placeholder="1883"
              value={data.mqttPort || ""}
              onChange={(e) => onChange({ mqttPort: parseInt(e.target.value) || 1883 })}
              className={errors.mqttPort ? "border-destructive" : ""}
            />
            {errors.mqttPort && <p className="text-sm text-destructive">{errors.mqttPort}</p>}
          </div>
        </div>
      </div>

      {/* 文件服务器 */}
      <div className="space-y-2">
        <Label htmlFor="fileServerHost">文件服务器地址</Label>
        <Input
          id="fileServerHost"
          placeholder="例如：http://fileserver.example.com:8080"
          value={data.fileServerHost || ""}
          onChange={(e) => onChange({ fileServerHost: e.target.value })}
          className={errors.fileServerHost ? "border-destructive" : ""}
        />
        {errors.fileServerHost && (
          <p className="text-sm text-destructive">{errors.fileServerHost}</p>
        )}
      </div>

      {/* 负责的数据库编号 */}
      <div className="space-y-2">
        <Label htmlFor="locationDbs">负责的数据库编号</Label>
        <Input
          id="locationDbs"
          placeholder="例如：7999,8001,8002（逗号分隔）"
          value={data.locationDbs || ""}
          onChange={(e) => onChange({ locationDbs: e.target.value })}
        />
        <p className="text-xs text-muted-foreground">
          输入该环境负责的数据库编号，多个编号用逗号分隔
        </p>
      </div>

      {/* 高级配置 */}
      <div className="space-y-4">
        <h3 className="text-sm font-medium">高级配置</h3>
        
        <div className="grid grid-cols-2 gap-4">
          <div className="space-y-2">
            <Label htmlFor="reconnectInitialMs">重连初始间隔（毫秒）</Label>
            <Input
              id="reconnectInitialMs"
              type="number"
              placeholder="1000"
              value={data.reconnectInitialMs || ""}
              onChange={(e) =>
                onChange({ reconnectInitialMs: parseInt(e.target.value) || 1000 })
              }
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="reconnectMaxMs">重连最大间隔（毫秒）</Label>
            <Input
              id="reconnectMaxMs"
              type="number"
              placeholder="30000"
              value={data.reconnectMaxMs || ""}
              onChange={(e) =>
                onChange({ reconnectMaxMs: parseInt(e.target.value) || 30000 })
              }
            />
          </div>
        </div>
      </div>

      {/* 下一步按钮 */}
      <div className="flex justify-end">
        <Button onClick={handleNext}>
          下一步
          <ChevronRight className="h-4 w-4 ml-2" />
        </Button>
      </div>
    </div>
  )
}
