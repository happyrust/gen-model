"use client"

import { useState, useEffect } from "react"
import { ChevronDown, ChevronRight, Terminal, X } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible"
import { cn } from "@/lib/utils"

interface LogEntry {
  timestamp: string
  level: "info" | "warn" | "error" | "success"
  message: string
}

interface DatabaseStartupLogProps {
  isVisible: boolean
  onClose: () => void
  instanceKey: string
}

export function DatabaseStartupLog({ isVisible, onClose, instanceKey }: DatabaseStartupLogProps) {
  const [isOpen, setIsOpen] = useState(true)
  const [logs, setLogs] = useState<LogEntry[]>([])
  const [isPolling, setIsPolling] = useState(false)

  // 轮询获取日志
  useEffect(() => {
    if (!isVisible || !instanceKey) return

    setIsPolling(true)
    const pollLogs = async () => {
      try {
        const baseUrl = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "") || ""
        const response = await fetch(
          `${baseUrl}/api/database/startup/logs?instance=${encodeURIComponent(instanceKey)}`
        )
        const data = await response.json()

        if (data.success && data.logs) {
          setLogs(data.logs)
        }
      } catch (error) {
        console.error("获取日志失败:", error)
      }
    }

    // 立即获取一次
    pollLogs()

    // 每2秒轮询一次
    const interval = setInterval(pollLogs, 2000)

    return () => {
      clearInterval(interval)
      setIsPolling(false)
    }
  }, [isVisible, instanceKey])

  if (!isVisible) return null

  const getLogLevelColor = (level: LogEntry["level"]) => {
    switch (level) {
      case "error":
        return "text-red-500"
      case "warn":
        return "text-yellow-500"
      case "success":
        return "text-green-500"
      case "info":
      default:
        return "text-blue-500"
    }
  }

  const getLogLevelIcon = (level: LogEntry["level"]) => {
    switch (level) {
      case "error":
        return "❌"
      case "warn":
        return "⚠️"
      case "success":
        return "✅"
      case "info":
      default:
        return "ℹ️"
    }
  }

  return (
    <div className="fixed bottom-4 right-4 w-96 max-w-[calc(100vw-2rem)] z-50">
      <Collapsible open={isOpen} onOpenChange={setIsOpen}>
        <div className="bg-white border border-gray-200 rounded-lg shadow-lg">
          {/* 头部 */}
          <CollapsibleTrigger asChild>
            <div className="flex items-center justify-between p-3 border-b border-gray-200 cursor-pointer hover:bg-gray-50">
              <div className="flex items-center gap-2">
                <Terminal className="h-4 w-4 text-blue-500" />
                <span className="font-medium text-sm">数据库启动日志</span>
                {isPolling && (
                  <div className="w-2 h-2 bg-blue-500 rounded-full animate-pulse" />
                )}
              </div>
              <div className="flex items-center gap-1">
                {isOpen ? (
                  <ChevronDown className="h-4 w-4 text-gray-500" />
                ) : (
                  <ChevronRight className="h-4 w-4 text-gray-500" />
                )}
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={(e) => {
                    e.stopPropagation()
                    onClose()
                  }}
                  className="h-6 w-6 p-0"
                >
                  <X className="h-3 w-3" />
                </Button>
              </div>
            </div>
          </CollapsibleTrigger>

          {/* 日志内容 */}
          <CollapsibleContent>
            <div className="max-h-64 overflow-y-auto">
              {logs.length === 0 ? (
                <div className="p-4 text-center text-gray-500 text-sm">
                  等待日志输出...
                </div>
              ) : (
                <div className="p-3 space-y-1">
                  {logs.map((log, index) => (
                    <div
                      key={index}
                      className="flex items-start gap-2 text-xs font-mono"
                    >
                      <span className="text-gray-400 shrink-0">
                        {log.timestamp}
                      </span>
                      <span className="shrink-0">
                        {getLogLevelIcon(log.level)}
                      </span>
                      <span
                        className={cn(
                          "flex-1 wrap-break-word",
                          getLogLevelColor(log.level)
                        )}
                      >
                        {log.message}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </CollapsibleContent>
        </div>
      </Collapsible>
    </div>
  )
}
