'use client'

import { useState, useEffect, useMemo, useRef } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { 
  Search, 
  Download, 
  RefreshCw, 
  Filter,
  AlertCircle,
  CheckCircle,
  Clock,
  XCircle,
} from 'lucide-react'
import { useToast } from '@/hooks/use-toast'

interface LogEntry {
  id: string
  task_id?: string
  env_id?: string
  source_env?: string
  target_site?: string
  site_id?: string
  direction?: string
  file_path?: string
  file_size?: number
  record_count?: number
  status: string
  error_message?: string
  notes?: string
  started_at?: string
  completed_at?: string
  created_at: string
  updated_at: string
}

export default function LogsPage() {
  const [logs, setLogs] = useState<LogEntry[]>([])
  const [filteredLogs, setFilteredLogs] = useState<LogEntry[]>([])
  const [selectedLog, setSelectedLog] = useState<LogEntry | null>(null)
  const [isLoading, setIsLoading] = useState(false)
  const [isDetailOpen, setIsDetailOpen] = useState(false)
  
  // 筛选条件
  const [searchTerm, setSearchTerm] = useState('')
  const [statusFilter, setStatusFilter] = useState<string>('all')
  const [envFilter, setEnvFilter] = useState<string>('all')
  const [directionFilter, setDirectionFilter] = useState<string>('all')
  
  const { toast } = useToast()

  // 加载日志
  const loadLogs = async () => {
    setIsLoading(true)
    try {
      const params = new URLSearchParams()
      if (statusFilter !== 'all') params.append('status', statusFilter)
      if (envFilter !== 'all') params.append('env_id', envFilter)
      if (directionFilter !== 'all') params.append('direction', directionFilter)
      params.append('limit', '1000')

      const response = await fetch(`/api/remote-sync/logs?${params}`)
      if (!response.ok) throw new Error('加载日志失败')
      
      const result = await response.json()
      if (result.status === 'success' && result.items) {
        setLogs(result.items)
      }
    } catch (error) {
      toast({
        title: '加载失败',
        description: error instanceof Error ? error.message : '未知错误',
        variant: 'destructive',
      })
    } finally {
      setIsLoading(false)
    }
  }

  // 应用筛选
  useEffect(() => {
    let filtered = logs

    // 搜索筛选
    if (searchTerm) {
      const term = searchTerm.toLowerCase()
      filtered = filtered.filter(log => 
        log.file_path?.toLowerCase().includes(term) ||
        log.error_message?.toLowerCase().includes(term) ||
        log.target_site?.toLowerCase().includes(term) ||
        log.id.toLowerCase().includes(term)
      )
    }

    setFilteredLogs(filtered)
  }, [logs, searchTerm])

  // 导出日志
  const exportLogs = (format: 'csv' | 'json') => {
    if (filteredLogs.length === 0) {
      toast({
        title: '导出失败',
        description: '没有可导出的日志',
        variant: 'destructive',
      })
      return
    }

    const exportData = filteredLogs.slice(0, 10000) // 限制 10000 条

    if (format === 'csv') {
      const csvContent = [
        ['ID', '状态', '文件路径', '大小', '记录数', '方向', '错误信息', '创建时间'],
        ...exportData.map(log => [
          log.id,
          log.status,
          log.file_path || '',
          log.file_size || 0,
          log.record_count || 0,
          log.direction || '',
          log.error_message || '',
          log.created_at,
        ])
      ].map(row => row.join(',')).join('\n')

      const blob = new Blob([csvContent], { type: 'text/csv;charset=utf-8;' })
      const link = document.createElement('a')
      link.href = URL.createObjectURL(blob)
      link.download = `logs-${new Date().toISOString()}.csv`
      link.click()
    } else {
      const jsonContent = JSON.stringify(exportData, null, 2)
      const blob = new Blob([jsonContent], { type: 'application/json' })
      const link = document.createElement('a')
      link.href = URL.createObjectURL(blob)
      link.download = `logs-${new Date().toISOString()}.json`
      link.click()
    }

    toast({
      title: '导出成功',
      description: `已导出 ${exportData.length} 条日志`,
    })
  }

  // 获取状态图标和颜色
  const getStatusBadge = (status: string) => {
    switch (status) {
      case 'completed':
        return <Badge className="bg-green-500"><CheckCircle className="w-3 h-3 mr-1" />完成</Badge>
      case 'failed':
        return <Badge variant="destructive"><XCircle className="w-3 h-3 mr-1" />失败</Badge>
      case 'running':
        return <Badge variant="secondary"><Clock className="w-3 h-3 mr-1" />运行中</Badge>
      default:
        return <Badge variant="outline">{status}</Badge>
    }
  }

  // 高亮错误关键词
  const highlightErrors = (text: string) => {
    if (!text) return text
    const errorKeywords = ['error', 'failed', 'timeout', 'exception', '失败', '错误', '超时']
    let highlighted = text
    errorKeywords.forEach(keyword => {
      const regex = new RegExp(`(${keyword})`, 'gi')
      highlighted = highlighted.replace(regex, '<mark class="bg-red-200">$1</mark>')
    })
    return highlighted
  }

  // 格式化文件大小
  const formatBytes = (bytes?: number) => {
    if (!bytes) return 'N/A'
    if (bytes < 1024) return `${bytes} B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  }

  // 虚拟滚动容器引用
  const parentRef = useRef<HTMLDivElement>(null)

  // 虚拟滚动配置
  const rowVirtualizer = useVirtualizer({
    count: filteredLogs.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 60,
    overscan: 10,
  })

  useEffect(() => {
    loadLogs()
  }, [statusFilter, envFilter, directionFilter])

  return (
    <div className="min-h-screen bg-background p-8">
      <div className="max-w-7xl mx-auto space-y-6">
        {/* Header */}
        <div className="flex items-center justify-between">
          <div>
            <h1 className="text-3xl font-bold tracking-tight">日志查询</h1>
            <p className="text-muted-foreground mt-1">
              查询和分析同步操作日志
            </p>
          </div>
          <div className="flex items-center gap-2">
            <Button onClick={loadLogs} variant="outline" size="sm" disabled={isLoading}>
              <RefreshCw className="w-4 h-4 mr-2" />
              刷新
            </Button>
            <Button onClick={() => exportLogs('csv')} variant="outline" size="sm">
              <Download className="w-4 h-4 mr-2" />
              导出 CSV
            </Button>
            <Button onClick={() => exportLogs('json')} variant="outline" size="sm">
              <Download className="w-4 h-4 mr-2" />
              导出 JSON
            </Button>
          </div>
        </div>

        {/* 筛选器 */}
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Filter className="w-5 h-5" />
              筛选条件
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="grid gap-4 md:grid-cols-4">
              <div>
                <label className="text-sm font-medium mb-2 block">搜索</label>
                <div className="relative">
                  <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 w-4 h-4 text-gray-400" />
                  <Input
                    placeholder="搜索文件路径、错误信息..."
                    value={searchTerm}
                    onChange={(e) => setSearchTerm(e.target.value)}
                    className="pl-10"
                  />
                </div>
              </div>

              <div>
                <label className="text-sm font-medium mb-2 block">状态</label>
                <Select value={statusFilter} onValueChange={setStatusFilter}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">全部</SelectItem>
                    <SelectItem value="completed">完成</SelectItem>
                    <SelectItem value="failed">失败</SelectItem>
                    <SelectItem value="running">运行中</SelectItem>
                  </SelectContent>
                </Select>
              </div>

              <div>
                <label className="text-sm font-medium mb-2 block">环境</label>
                <Select value={envFilter} onValueChange={setEnvFilter}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">全部</SelectItem>
                    {/* 动态加载环境列表 */}
                  </SelectContent>
                </Select>
              </div>

              <div>
                <label className="text-sm font-medium mb-2 block">方向</label>
                <Select value={directionFilter} onValueChange={setDirectionFilter}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">全部</SelectItem>
                    <SelectItem value="push">推送</SelectItem>
                    <SelectItem value="pull">拉取</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>
          </CardContent>
        </Card>

        {/* 日志列表 */}
        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <CardTitle>日志列表</CardTitle>
              <span className="text-sm text-muted-foreground">
                共 {filteredLogs.length} 条记录
              </span>
            </div>
          </CardHeader>
          <CardContent>
            <div 
              ref={parentRef}
              className="h-[600px] overflow-auto border rounded-lg"
            >
              <div
                style={{
                  height: `${rowVirtualizer.getTotalSize()}px`,
                  width: '100%',
                  position: 'relative',
                }}
              >
                {rowVirtualizer.getVirtualItems().map((virtualRow) => {
                  const log = filteredLogs[virtualRow.index]
                  return (
                    <div
                      key={virtualRow.key}
                      style={{
                        position: 'absolute',
                        top: 0,
                        left: 0,
                        width: '100%',
                        height: `${virtualRow.size}px`,
                        transform: `translateY(${virtualRow.start}px)`,
                      }}
                      className="border-b hover:bg-gray-50 cursor-pointer p-4"
                      onClick={() => {
                        setSelectedLog(log)
                        setIsDetailOpen(true)
                      }}
                    >
                      <div className="flex items-center justify-between">
                        <div className="flex-1 min-w-0">
                          <div className="flex items-center gap-2 mb-1">
                            {getStatusBadge(log.status)}
                            <span className="text-sm font-medium truncate">
                              {log.file_path || log.id}
                            </span>
                          </div>
                          <div className="text-xs text-gray-500 flex items-center gap-4">
                            <span>{formatBytes(log.file_size)}</span>
                            <span>{log.record_count || 0} 条记录</span>
                            <span>{log.direction || 'N/A'}</span>
                            <span>{new Date(log.created_at).toLocaleString()}</span>
                          </div>
                          {log.error_message && (
                            <div 
                              className="text-xs text-red-600 mt-1 truncate"
                              dangerouslySetInnerHTML={{ 
                                __html: highlightErrors(log.error_message) 
                              }}
                            />
                          )}
                        </div>
                      </div>
                    </div>
                  )
                })}
              </div>
            </div>
          </CardContent>
        </Card>
      </div>

      {/* 日志详情抽屉 */}
      <Sheet open={isDetailOpen} onOpenChange={setIsDetailOpen}>
        <SheetContent className="w-[600px] sm:max-w-[600px] overflow-y-auto">
          <SheetHeader>
            <SheetTitle>日志详情</SheetTitle>
            <SheetDescription>
              查看完整的日志信息
            </SheetDescription>
          </SheetHeader>
          {selectedLog && (
            <div className="mt-6 space-y-4">
              <div>
                <label className="text-sm font-medium">状态</label>
                <div className="mt-1">{getStatusBadge(selectedLog.status)}</div>
              </div>
              
              <div>
                <label className="text-sm font-medium">文件路径</label>
                <div className="mt-1 text-sm break-all">{selectedLog.file_path || 'N/A'}</div>
              </div>

              <div className="grid grid-cols-2 gap-4">
                <div>
                  <label className="text-sm font-medium">文件大小</label>
                  <div className="mt-1 text-sm">{formatBytes(selectedLog.file_size)}</div>
                </div>
                <div>
                  <label className="text-sm font-medium">记录数</label>
                  <div className="mt-1 text-sm">{selectedLog.record_count || 0}</div>
                </div>
              </div>

              <div className="grid grid-cols-2 gap-4">
                <div>
                  <label className="text-sm font-medium">方向</label>
                  <div className="mt-1 text-sm">{selectedLog.direction || 'N/A'}</div>
                </div>
                <div>
                  <label className="text-sm font-medium">环境</label>
                  <div className="mt-1 text-sm">{selectedLog.env_id || 'N/A'}</div>
                </div>
              </div>

              {selectedLog.error_message && (
                <div>
                  <label className="text-sm font-medium text-red-600">错误信息</label>
                  <div 
                    className="mt-1 text-sm p-3 bg-red-50 rounded border border-red-200"
                    dangerouslySetInnerHTML={{ 
                      __html: highlightErrors(selectedLog.error_message) 
                    }}
                  />
                </div>
              )}

              <div>
                <label className="text-sm font-medium">创建时间</label>
                <div className="mt-1 text-sm">
                  {new Date(selectedLog.created_at).toLocaleString()}
                </div>
              </div>

              {selectedLog.started_at && (
                <div>
                  <label className="text-sm font-medium">开始时间</label>
                  <div className="mt-1 text-sm">
                    {new Date(selectedLog.started_at).toLocaleString()}
                  </div>
                </div>
              )}

              {selectedLog.completed_at && (
                <div>
                  <label className="text-sm font-medium">完成时间</label>
                  <div className="mt-1 text-sm">
                    {new Date(selectedLog.completed_at).toLocaleString()}
                  </div>
                </div>
              )}

              <div>
                <label className="text-sm font-medium">任务 ID</label>
                <div className="mt-1 text-sm font-mono text-xs break-all">
                  {selectedLog.task_id || selectedLog.id}
                </div>
              </div>
            </div>
          )}
        </SheetContent>
      </Sheet>
    </div>
  )
}
