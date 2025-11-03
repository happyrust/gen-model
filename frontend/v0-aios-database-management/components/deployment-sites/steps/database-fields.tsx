import { useState, useEffect } from "react"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Button } from "@/components/ui/button"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Database, HardDrive, Code, Settings2, Play, Square, Loader2, AlertTriangle } from "lucide-react"
import { DB_TYPES, MODULES } from "../site-config"
import type { CreateSiteFormData } from "@/hooks/use-create-site-form"
import { DatabaseStartupLog } from "@/components/database-startup-log"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"

interface FieldsProps {
  formData: CreateSiteFormData
  onConfigChange: (field: string, value: any) => void
}

export function BasicDatabaseFields({ formData, onConfigChange }: FieldsProps) {
  return (
    <div className="grid grid-cols-2 gap-3">
      <div className="space-y-2">
        <Label className="text-sm">数据库类型</Label>
        <Select
          value={formData.config.db_type}
          onValueChange={(value) => onConfigChange("db_type", value)}
        >
          <SelectTrigger className="h-9">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {DB_TYPES.map(type => (
              <SelectItem key={type} value={type}>{type.toUpperCase()}</SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="space-y-2">
        <Label className="text-sm">项目代码</Label>
        <Input
          type="number"
          className="h-9"
          value={formData.config.project_code}
          onChange={(e) => onConfigChange("project_code", parseInt(e.target.value) || 0)}
        />
      </div>
    </div>
  )
}

export function DatabaseConnectionFields({ formData, onConfigChange }: FieldsProps) {
  const [dbStatus, setDbStatus] = useState<"unknown" | "starting" | "running" | "stopped">("unknown")
  const [isLoading, setIsLoading] = useState(false)
  const [showLogs, setShowLogs] = useState(false)
  const [errorDialog, setErrorDialog] = useState<{
    isOpen: boolean
    title: string
    message: string
  }>({
    isOpen: false,
    title: "",
    message: ""
  })

  useEffect(() => {
    setDbStatus("unknown")
  }, [formData.config.db_ip, formData.config.db_port])

  const checkDbStatus = async () => {
    setIsLoading(true)
    try {
      const baseUrl = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "") || ""
      const response = await fetch(
        `${baseUrl}/api/database/startup/status?ip=${formData.config.db_ip}&port=${formData.config.db_port}`
      )
      const data = await response.json()

      console.log("数据库状态检查结果:", data)

      if (data.success) {
        const status = data.status
        if (status === "Running") {
          setDbStatus("running")
        } else if (status === "Starting") {
          setDbStatus("starting")
        } else {
          setDbStatus("stopped")
        }
      } else {
        setDbStatus("stopped")
      }
    } catch (err) {
      console.error("检查数据库状态失败:", err)
      setDbStatus("stopped")
    } finally {
      setIsLoading(false)
    }
  }

  const startDatabase = async () => {
    setIsLoading(true)
    setShowLogs(true) // 显示日志窗口
    
    try {
      const baseUrl = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "") || ""
      const response = await fetch(`${baseUrl}/api/database/startup/start`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          ip: formData.config.db_ip,
          port: parseInt(formData.config.db_port),
          user: formData.config.db_user,
          password: formData.config.db_password,
          dbFile: "surreal.db"
        })
      })
      const data = await response.json()

      if (data.success) {
        setDbStatus("starting")
        // 轮询检查状态，最多检查 10 次
        let attempts = 0
        const maxAttempts = 10
        const pollStatus = async () => {
          attempts++
          await new Promise(resolve => setTimeout(resolve, 1000))

          const statusResponse = await fetch(
            `${baseUrl}/api/database/startup/status?ip=${formData.config.db_ip}&port=${formData.config.db_port}`
          )
          const statusData = await statusResponse.json()

          console.log(`轮询状态 (${attempts}/${maxAttempts}):`, statusData)

          if (statusData.success && statusData.status === "Running") {
            setDbStatus("running")
            setIsLoading(false)
          } else if (statusData.success && statusData.status === "Failed") {
            // 启动失败，显示错误弹窗
            setErrorDialog({
              isOpen: true,
              title: "数据库启动失败",
              message: statusData.error_message || "启动过程中发生未知错误"
            })
            setDbStatus("stopped")
            setIsLoading(false)
          } else if (attempts < maxAttempts) {
            pollStatus()
          } else {
            // 超时，显示错误弹窗
            setErrorDialog({
              isOpen: true,
              title: "数据库启动超时",
              message: "数据库启动时间过长，请检查配置或重试"
            })
            setDbStatus("stopped")
            setIsLoading(false)
          }
        }
        pollStatus()
      } else {
        // 启动请求失败
        setErrorDialog({
          isOpen: true,
          title: "启动请求失败",
          message: data.error || "无法提交启动请求"
        })
        setIsLoading(false)
      }
    } catch (err) {
      console.error("启动数据库失败:", err)
      setErrorDialog({
        isOpen: true,
        title: "网络错误",
        message: `启动数据库时发生网络错误: ${err}`
      })
      setIsLoading(false)
    }
  }

  const stopDatabase = async () => {
    setIsLoading(true)
    try {
      const baseUrl = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "") || ""
      const response = await fetch(`${baseUrl}/api/database/startup/stop`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          ip: formData.config.db_ip,
          port: parseInt(formData.config.db_port)
        })
      })
      const data = await response.json()

      if (data.success) {
        setDbStatus("stopped")
      }
    } catch (err) {
      console.error("停止数据库失败:", err)
    } finally {
      setIsLoading(false)
    }
  }

  return (
    <>
      <div className="grid grid-cols-2 gap-3">
        <div className="space-y-2">
          <Label className="text-sm">数据库 IP</Label>
          <Input
            className="h-9"
            value={formData.config.db_ip}
            onChange={(e) => onConfigChange("db_ip", e.target.value)}
          />
        </div>

        <div className="space-y-2">
          <Label className="text-sm">端口</Label>
          <Input
            className="h-9"
            value={formData.config.db_port}
            onChange={(e) => onConfigChange("db_port", e.target.value)}
          />
        </div>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div className="space-y-2">
          <Label className="text-sm">用户名</Label>
          <Input
            className="h-9"
            value={formData.config.db_user}
            onChange={(e) => onConfigChange("db_user", e.target.value)}
          />
        </div>

        <div className="space-y-2">
          <Label className="text-sm">密码</Label>
          <Input
            type="password"
            className="h-9"
            value={formData.config.db_password}
            onChange={(e) => onConfigChange("db_password", e.target.value)}
          />
        </div>
      </div>

      <div className="flex items-center gap-2 pt-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={checkDbStatus}
          disabled={isLoading}
        >
          {isLoading && <Loader2 className="h-3 w-3 mr-1 animate-spin" />}
          检查状态
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={startDatabase}
          disabled={isLoading || dbStatus === "running"}
        >
          <Play className="h-3 w-3 mr-1" />
          启动数据库
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={stopDatabase}
          disabled={isLoading || dbStatus === "stopped"}
        >
          <Square className="h-3 w-3 mr-1" />
          停止数据库
        </Button>
        {dbStatus !== "unknown" && (
          <span className={`text-xs ${dbStatus === "running" ? "text-success" : "text-muted-foreground"}`}>
            {dbStatus === "running" ? "● 运行中" : dbStatus === "starting" ? "○ 启动中" : "○ 已停止"}
          </span>
        )}
      </div>

      {/* 日志显示组件 */}
      <DatabaseStartupLog
        isVisible={showLogs}
        onClose={() => setShowLogs(false)}
        instanceKey={`${formData.config.db_ip}:${formData.config.db_port}`}
      />

      {/* 错误弹窗 */}
      <AlertDialog open={errorDialog.isOpen} onOpenChange={(open) => 
        setErrorDialog(prev => ({ ...prev, isOpen: open }))
      }>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle className="flex items-center gap-2">
              <AlertTriangle className="h-5 w-5 text-red-500" />
              {errorDialog.title}
            </AlertDialogTitle>
            <AlertDialogDescription className="whitespace-pre-wrap">
              {errorDialog.message}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogAction onClick={() => setErrorDialog(prev => ({ ...prev, isOpen: false }))}>
              确定
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}

export function AdvancedConfigFields({ formData, onConfigChange }: FieldsProps) {
  return (
    <div className="border-t pt-3 space-y-3">
      <div className="grid grid-cols-2 gap-3">
        <div className="space-y-2">
          <Label className="text-sm">MDB 名称</Label>
          <Input
            className="h-9"
            value={formData.config.mdb_name}
            onChange={(e) => onConfigChange("mdb_name", e.target.value)}
          />
        </div>

        <div className="space-y-2">
          <Label className="text-sm">模块</Label>
          <Select
            value={formData.config.module}
            onValueChange={(value) => onConfigChange("module", value)}
          >
            <SelectTrigger className="h-9">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {MODULES.map(mod => (
                <SelectItem key={mod} value={mod}>{mod}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>

      <GenerationOptions formData={formData} onConfigChange={onConfigChange} />

      <div className="grid grid-cols-2 gap-3">
        <div className="space-y-2">
          <Label className="text-sm">网格容差比率</Label>
          <Input
            type="number"
            step="0.1"
            className="h-9"
            value={formData.config.mesh_tol_ratio}
            onChange={(e) => onConfigChange("mesh_tol_ratio", parseFloat(e.target.value) || 3.0)}
          />
        </div>

        <div className="space-y-2">
          <Label className="text-sm">房间关键字</Label>
          <Input
            className="h-9"
            value={formData.config.room_keyword}
            onChange={(e) => onConfigChange("room_keyword", e.target.value)}
          />
        </div>
      </div>
    </div>
  )
}

export function GenerationOptions({ formData, onConfigChange }: FieldsProps) {
  return (
    <div className="space-y-3">
      <Label className="text-sm">生成选项</Label>
      <div className="space-y-2">
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={formData.config.gen_model}
            onChange={(e) => onConfigChange("gen_model", e.target.checked)}
            className="rounded border-gray-300"
          />
          <Database className="h-4 w-4 text-muted-foreground" />
          生成 3D 模型
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={formData.config.gen_mesh}
            onChange={(e) => onConfigChange("gen_mesh", e.target.checked)}
            className="rounded border-gray-300"
          />
          <HardDrive className="h-4 w-4 text-muted-foreground" />
          生成网格数据
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={formData.config.gen_spatial_tree}
            onChange={(e) => onConfigChange("gen_spatial_tree", e.target.checked)}
            className="rounded border-gray-300"
          />
          <Code className="h-4 w-4 text-muted-foreground" />
          生成空间树
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={formData.config.apply_boolean_operation}
            onChange={(e) => onConfigChange("apply_boolean_operation", e.target.checked)}
            className="rounded border-gray-300"
          />
          <Settings2 className="h-4 w-4 text-muted-foreground" />
          应用布尔运算
        </label>
      </div>
    </div>
  )
}