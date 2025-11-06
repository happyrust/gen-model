"use client"

import { useState, useEffect, useRef, useCallback } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Badge } from "@/components/ui/badge"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Search,
  Download,
  Filter,
  RefreshCw,
  Copy,
  ChevronDown,
  ChevronUp,
  AlertCircle,
  Info,
  AlertTriangle,
} from "lucide-react"
import { useTaskLogs } from "@/hooks/use-task-logs"
import { useWebSocket } from "@/hooks/use-websocket"
import type { LogEntry as LogEntryType, LogFilters as LogFiltersType } from "@/types/task-logs"

interface LogViewerProps {
  taskId?: string
  autoScroll?: boolean
  showTimestamp?: boolean
  maxLines?: number
}

interface LogEntryProps {
  log: LogEntryType
  showTimestamp: boolean
  onExpand?: (logId: string) => void
  onCopy?: (content: string) => void
}

interface LogFiltersProps {
  filters: LogFiltersType
  onChange: (filters: LogFiltersType) => void
}

interface LogSearchProps {
  onSearch: (value: string) => void
  placeholder?: string
  debounceMs?: number
}

interface LogDownloaderProps {
  logs: LogEntryType[]
  onClose?: () => void
}

function LogSearch({ onSearch, placeholder, debounceMs = 0 }: LogSearchProps) {
  const [value, setValue] = useState("")
  const timeoutRef = useRef<NodeJS.Timeout | null>(null)

  const handleChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const nextValue = event.target.value
    setValue(nextValue)
    if (timeoutRef.current) {
      clearTimeout(timeoutRef.current)
    }
    if (debounceMs > 0) {
      timeoutRef.current = setTimeout(() => {
        onSearch(nextValue)
      }, debounceMs)
    } else {
      onSearch(nextValue)
    }
  }

  useEffect(() => {
    return () => {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current)
      }
    }
  }, [])

  return (
    <div className="relative">
      <Search className="absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
      <Input
        value={value}
        onChange={handleChange}
        placeholder={placeholder ?? "搜索日志"}
        className="pl-9"
      />
    </div>
  )
}

function LogFilters({ filters, onChange }: LogFiltersProps) {
  return (
    <div className="grid gap-4 rounded-lg border border-border/60 bg-muted/30 p-4 md:grid-cols-3">
      <div className="space-y-1">
        <span className="text-xs text-muted-foreground">日志级别</span>
        <Select
          value={filters.level}
          onValueChange={(value) =>
            onChange({
              ...filters,
              level: value as LogFiltersType["level"],
            })
          }
        >
          <SelectTrigger>
            <SelectValue placeholder="选择级别" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">全部</SelectItem>
            <SelectItem value="info">信息</SelectItem>
            <SelectItem value="warn">警告</SelectItem>
            <SelectItem value="error">错误</SelectItem>
            <SelectItem value="debug">调试</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <div className="space-y-1">
        <span className="text-xs text-muted-foreground">任务</span>
        <Input
          value={filters.taskId === "all" ? "" : filters.taskId}
          placeholder="任务 ID (留空表示全部)"
          onChange={(event) =>
            onChange({
              ...filters,
              taskId: event.target.value.trim() || "all",
            })
          }
        />
      </div>
      <div className="space-y-1">
        <span className="text-xs text-muted-foreground">关键字</span>
        <Input
          value={filters.search}
          placeholder="输入关键字过滤"
          onChange={(event) =>
            onChange({
              ...filters,
              search: event.target.value,
            })
          }
        />
      </div>
    </div>
  )
}

function LogEntry({ log, showTimestamp, onExpand, onCopy }: LogEntryProps) {
  const levelColors: Record<LogEntryType["level"], string> = {
    info: "text-blue-600",
    warn: "text-yellow-600",
    warning: "text-yellow-600",
    error: "text-red-600",
    debug: "text-muted-foreground",
    critical: "text-orange-600",
  }

  return (
    <div className="flex items-start justify-between gap-3 rounded border border-border/40 bg-background/80 p-3">
      <Badge variant="outline" className={levelColors[log.level]}>
        {log.level.toUpperCase()}
      </Badge>
      <div className="flex-1 space-y-1">
        {showTimestamp && <p className="text-xs text-muted-foreground">{new Date(log.timestamp).toLocaleString()}</p>}
        <p className="text-sm leading-relaxed text-foreground">{log.message}</p>
        {log.source && <p className="text-xs text-muted-foreground">来源: {log.source}</p>}
      </div>
      {(onExpand || onCopy) && (
        <div className="flex flex-col items-end gap-2">
          {onExpand && (
            <Button size="sm" variant="ghost" className="h-7 px-2 text-xs" onClick={() => onExpand(log.id)}>
              详情
            </Button>
          )}
          {onCopy && (
            <Button
              size="sm"
              variant="ghost"
              className="h-7 px-2 text-xs"
              onClick={() => onCopy(log.message)}
            >
              复制
            </Button>
          )}
        </div>
      )}
    </div>
  )
}

function LogDownloader({ logs, onClose }: LogDownloaderProps) {
  const handleDownload = () => {
    const blob = new Blob(
      [logs.map((log) => `[${log.timestamp}] [${log.level}] ${log.message}`).join("\n")],
      { type: "text/plain;charset=utf-8" }
    )
    const url = URL.createObjectURL(blob)
    const link = document.createElement("a")
    link.href = url
    link.download = `task-logs-${Date.now()}.txt`
    document.body.appendChild(link)
    link.click()
    document.body.removeChild(link)
    URL.revokeObjectURL(url)
    onClose?.()
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-sm">导出日志</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <p className="text-xs text-muted-foreground">导出当前筛选结果到文本文件。</p>
        <Button size="sm" onClick={handleDownload} className="gap-2">
          <Download className="h-4 w-4" />
          下载日志
        </Button>
        <Button size="sm" variant="outline" onClick={onClose}>
          取消
        </Button>
      </CardContent>
    </Card>
  )
}

export function LogViewer({ 
  taskId, 
  autoScroll = true, 
  showTimestamp = true,
  maxLines = 1000 
}: LogViewerProps) {
  const [showFilters, setShowFilters] = useState(false)
  const [showDownloader, setShowDownloader] = useState(false)
  const [isAutoScroll, setIsAutoScroll] = useState(autoScroll)
  const scrollAreaRef = useRef<HTMLDivElement>(null)

  const {
    logs,
    loading,
    error,
    filters,
    pagination,
    loadLogs,
    refreshLogs,
    setFilters: updateFilters,
    searchLogs,
  } = useTaskLogs(taskId)

  // WebSocket连接用于实时日志
  const { isConnected, lastMessage } = useWebSocket(
    taskId ? `/ws/tasks/${taskId}/logs` : '/ws/tasks/logs'
  )

  // 加载日志数据
  useEffect(() => {
    void loadLogs()
  }, [loadLogs, filters])

  // 处理WebSocket消息
  useEffect(() => {
    if (lastMessage && lastMessage.type === 'log_entry') {
      // 实时添加新日志
      refreshLogs()
    }
  }, [lastMessage, refreshLogs])

  // 自动滚动到底部
  useEffect(() => {
    if (isAutoScroll && scrollAreaRef.current) {
      const scrollElement = scrollAreaRef.current.querySelector('[data-radix-scroll-area-viewport]')
      if (scrollElement) {
        scrollElement.scrollTop = scrollElement.scrollHeight
      }
    }
  }, [logs, isAutoScroll])

  // 处理过滤条件变化
  const handleFilterChange = useCallback(
    (newFilters: Partial<LogFiltersType>) => {
      updateFilters(newFilters)
    },
    [updateFilters]
  )

  // 处理搜索
  const handleSearch = useCallback(
    (searchQuery: string) => {
      searchLogs(searchQuery)
    },
    [searchLogs]
  )

  // 复制日志内容
  const handleCopyLogs = useCallback(() => {
    const logText = logs.map(log => 
      `[${log.timestamp}] [${log.level}] ${log.message}`
    ).join('\n')
    
    navigator.clipboard.writeText(logText).then(() => {
      // 可以添加toast提示
      console.log('日志已复制到剪贴板')
    })
  }, [logs])

  // 切换自动滚动
  const toggleAutoScroll = useCallback(() => {
    setIsAutoScroll(prev => !prev)
  }, [])

  // 获取显示的日志
  const filteredLogs = logs
  const limitedLogs = maxLines ? filteredLogs.slice(-maxLines) : filteredLogs

  return (
    <div className="space-y-4">
      {/* 头部控制栏 */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex items-center justify-between">
            <CardTitle className="text-lg">任务日志</CardTitle>
            <div className="flex items-center gap-2">
              <Badge variant={isConnected ? "default" : "secondary"}>
                {isConnected ? "实时连接" : "离线"}
              </Badge>
              <Button
                variant="outline"
                size="sm"
                onClick={toggleAutoScroll}
              >
                {isAutoScroll ? "关闭自动滚动" : "开启自动滚动"}
              </Button>
            </div>
          </div>
        </CardHeader>
        <CardContent className="space-y-4">
          {/* 搜索和过滤 */}
          <div className="flex items-center gap-2">
            <div className="flex-1">
              <LogSearch
                onSearch={handleSearch}
                placeholder="搜索日志内容..."
                debounceMs={300}
              />
            </div>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setShowFilters(!showFilters)}
            >
              <Filter className="h-4 w-4 mr-2" />
              过滤
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={refreshLogs}
              disabled={loading}
            >
              <RefreshCw className={`h-4 w-4 mr-2 ${loading ? 'animate-spin' : ''}`} />
              刷新
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={handleCopyLogs}
            >
              <Copy className="h-4 w-4 mr-2" />
              复制
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setShowDownloader(true)}
            >
              <Download className="h-4 w-4 mr-2" />
              导出
            </Button>
          </div>

          {/* 过滤条件 */}
          {showFilters && (
            <LogFilters
              filters={filters}
              onChange={handleFilterChange}
            />
          )}
        </CardContent>
      </Card>

      {/* 日志内容 */}
      <Card>
        <CardContent className="p-0">
          <ScrollArea 
            ref={scrollAreaRef}
            className="h-[600px] w-full"
          >
            <div className="p-4 space-y-1">
              {loading && logs.length === 0 ? (
                <div className="flex items-center justify-center py-8">
                  <RefreshCw className="h-4 w-4 mr-2 animate-spin" />
                  <span>加载日志中...</span>
                </div>
              ) : error ? (
                <div className="flex items-center justify-center py-8 text-red-600">
                  <AlertCircle className="h-4 w-4 mr-2" />
                  <span>{error}</span>
                </div>
              ) : limitedLogs.length === 0 ? (
                <div className="flex items-center justify-center py-8 text-muted-foreground">
                  <Info className="h-4 w-4 mr-2" />
                  <span>暂无日志数据</span>
                </div>
              ) : (
                limitedLogs.map((log, index) => (
                  <LogEntry
                    key={`${log.id}-${index}`}
                    log={log}
                    showTimestamp={showTimestamp}
                    onExpand={(logId) => {
                      console.log('Expand log:', logId)
                    }}
                    onCopy={(content) => {
                      navigator.clipboard.writeText(content)
                    }}
                  />
                ))
              )}
            </div>
          </ScrollArea>
        </CardContent>
      </Card>

      {/* 日志统计 */}
      <div className="flex items-center justify-between text-sm text-muted-foreground">
        <div className="flex items-center gap-4">
          <span>总计: {pagination.totalItems} 条</span>
          {filters.search && (
            <span>匹配: {filteredLogs.length} 条</span>
          )}
          {maxLines && limitedLogs.length === maxLines && (
            <span className="text-yellow-600">
              显示最近 {maxLines} 条日志
            </span>
          )}
        </div>
        <div className="flex items-center gap-2">
          <span>
            INFO: {filteredLogs.filter((l) => l.level === "info").length}
          </span>
          <span>
            WARN: {filteredLogs.filter((l) => l.level === "warn" || l.level === "warning").length}
          </span>
          <span>
            ERROR: {filteredLogs.filter((l) => l.level === "error").length}
          </span>
        </div>
      </div>

      {/* 下载对话框 */}
      {showDownloader && (
        <LogDownloader
          logs={limitedLogs}
          onClose={() => setShowDownloader(false)}
        />
      )}
    </div>
  )
}
