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
  AlertTriangle
} from "lucide-react"
import { LogEntry } from "./LogEntry"
import { LogFilters } from "./LogFilters"
import { LogSearch } from "./LogSearch"
import { LogDownloader } from "./LogDownloader"
import { useTaskLogs } from "@/hooks/use-task-logs"
import { useLogSearch } from "@/hooks/use-log-search"
import { useWebSocket } from "@/hooks/use-websocket"
import type { LogEntry as LogEntryType, LogFilters as LogFiltersType } from "@/types/task-logs"

interface LogViewerProps {
  taskId?: string
  autoScroll?: boolean
  showTimestamp?: boolean
  maxLines?: number
}

export function LogViewer({ 
  taskId, 
  autoScroll = true, 
  showTimestamp = true,
  maxLines = 1000 
}: LogViewerProps) {
  const [filters, setFilters] = useState<LogFiltersType>({
    level: 'all',
    search: '',
    dateRange: null,
    taskId: taskId || 'all'
  })
  
  const [showFilters, setShowFilters] = useState(false)
  const [showDownloader, setShowDownloader] = useState(false)
  const [isAutoScroll, setIsAutoScroll] = useState(autoScroll)
  const scrollAreaRef = useRef<HTMLDivElement>(null)
  const lastLogCountRef = useRef(0)

  const {
    logs,
    loading,
    error,
    loadLogs,
    refreshLogs
  } = useTaskLogs(taskId)

  const {
    query,
    results,
    isSearching,
    searchLogs
  } = useLogSearch()

  // WebSocket连接用于实时日志
  const { isConnected, lastMessage } = useWebSocket(
    taskId ? `/ws/tasks/${taskId}/logs` : '/ws/tasks/logs'
  )

  // 加载日志数据
  useEffect(() => {
    loadLogs(filters)
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
  const handleFilterChange = useCallback((newFilters: LogFiltersType) => {
    setFilters(prev => ({ ...prev, ...newFilters }))
  }, [])

  // 处理搜索
  const handleSearch = useCallback((searchQuery: string) => {
    if (searchQuery.trim()) {
      searchLogs(searchQuery)
    } else {
      setFilters(prev => ({ ...prev, search: '' }))
    }
  }, [searchLogs])

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
  const displayLogs = filters.search ? results : logs
  const filteredLogs = displayLogs.filter(log => {
    if (filters.level !== 'all' && log.level !== filters.level) {
      return false
    }
    if (filters.dateRange) {
      const logDate = new Date(log.timestamp)
      const [start, end] = filters.dateRange
      if (logDate < start || logDate > end) {
        return false
      }
    }
    return true
  })

  // 限制显示行数
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
              onFilterChange={handleFilterChange}
              availableLevels={['all', 'info', 'warn', 'error']}
              availableTasks={[]}
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
          <span>总计: {filteredLogs.length} 条</span>
          {filters.search && (
            <span>搜索结果: {results.length} 条</span>
          )}
          {maxLines && limitedLogs.length === maxLines && (
            <span className="text-yellow-600">
              显示最近 {maxLines} 条日志
            </span>
          )}
        </div>
        <div className="flex items-center gap-2">
          <span>INFO: {filteredLogs.filter(l => l.level === 'info').length}</span>
          <span>WARN: {filteredLogs.filter(l => l.level === 'warn').length}</span>
          <span>ERROR: {filteredLogs.filter(l => l.level === 'error').length}</span>
        </div>
      </div>

      {/* 下载对话框 */}
      {showDownloader && taskId && (
        <LogDownloader
          taskId={taskId}
          format="txt"
          dateRange={[new Date(Date.now() - 24 * 60 * 60 * 1000), new Date()]}
          onClose={() => setShowDownloader(false)}
        />
      )}
    </div>
  )
}
