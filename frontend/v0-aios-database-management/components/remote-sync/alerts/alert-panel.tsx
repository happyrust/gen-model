'use client'

import { useState, useEffect } from 'react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  AlertCircle,
  AlertTriangle,
  Info,
  XCircle,
  X,
  ExternalLink,
} from 'lucide-react'
import { useRouter } from 'next/navigation'

interface Alert {
  id: string
  level: 'info' | 'warning' | 'error' | 'critical'
  message: string
  timestamp: string
  read?: boolean
  actionUrl?: string
}

interface AlertPanelProps {
  maxAlerts?: number
  showHistory?: boolean
}

export function AlertPanel({ maxAlerts = 10, showHistory = true }: AlertPanelProps) {
  const [alerts, setAlerts] = useState<Alert[]>([])
  const [unreadCount, setUnreadCount] = useState(0)
  const router = useRouter()

  // 监听 SSE 事件
  useEffect(() => {
    const eventSource = new EventSource('/api/sync/events')

    eventSource.addEventListener('message', (event) => {
      try {
        const data = JSON.parse(event.data)
        
        // 处理告警事件
        if (data.type === 'Alert') {
          const newAlert: Alert = {
            id: `alert-${Date.now()}`,
            level: data.data.level as any,
            message: data.data.message,
            timestamp: data.data.timestamp,
            read: false,
          }
          
          setAlerts(prev => [newAlert, ...prev].slice(0, maxAlerts))
          setUnreadCount(prev => prev + 1)
        }
      } catch (error) {
        console.error('解析 SSE 事件失败:', error)
      }
    })

    return () => {
      eventSource.close()
    }
  }, [maxAlerts])

  // 获取告警图标和颜色
  const getAlertIcon = (level: string) => {
    switch (level) {
      case 'critical':
        return <XCircle className="w-5 h-5 text-red-600" />
      case 'error':
        return <AlertCircle className="w-5 h-5 text-red-500" />
      case 'warning':
        return <AlertTriangle className="w-5 h-5 text-yellow-500" />
      case 'info':
      default:
        return <Info className="w-5 h-5 text-blue-500" />
    }
  }

  const getAlertBadge = (level: string) => {
    switch (level) {
      case 'critical':
        return <Badge variant="destructive" className="bg-red-600">严重</Badge>
      case 'error':
        return <Badge variant="destructive">错误</Badge>
      case 'warning':
        return <Badge className="bg-yellow-500">警告</Badge>
      case 'info':
      default:
        return <Badge variant="secondary">信息</Badge>
    }
  }

  // 标记为已读
  const markAsRead = (id: string) => {
    setAlerts(prev => prev.map(alert => 
      alert.id === id ? { ...alert, read: true } : alert
    ))
    setUnreadCount(prev => Math.max(0, prev - 1))
  }

  // 删除告警
  const dismissAlert = (id: string) => {
    const alert = alerts.find(a => a.id === id)
    if (alert && !alert.read) {
      setUnreadCount(prev => Math.max(0, prev - 1))
    }
    setAlerts(prev => prev.filter(a => a.id !== id))
  }

  // 清空所有告警
  const clearAll = () => {
    setAlerts([])
    setUnreadCount(0)
  }

  // 跳转到相关页面
  const handleAlertClick = (alert: Alert) => {
    markAsRead(alert.id)
    if (alert.actionUrl) {
      router.push(alert.actionUrl)
    }
  }

  return (
    <Card>
      <CardHeader>
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <CardTitle>告警通知</CardTitle>
            {unreadCount > 0 && (
              <Badge variant="destructive" className="rounded-full">
                {unreadCount}
              </Badge>
            )}
          </div>
          {alerts.length > 0 && (
            <Button variant="ghost" size="sm" onClick={clearAll}>
              清空全部
            </Button>
          )}
        </div>
      </CardHeader>
      <CardContent>
        {alerts.length === 0 ? (
          <div className="text-center py-8 text-muted-foreground">
            <Info className="w-12 h-12 mx-auto mb-2 opacity-50" />
            <p>暂无告警</p>
          </div>
        ) : (
          <ScrollArea className="h-[400px]">
            <div className="space-y-3">
              {alerts.map((alert) => (
                <div
                  key={alert.id}
                  className={`
                    p-4 rounded-lg border transition-all
                    ${alert.read ? 'bg-gray-50 opacity-75' : 'bg-white'}
                    ${alert.actionUrl ? 'cursor-pointer hover:shadow-md' : ''}
                  `}
                  onClick={() => alert.actionUrl && handleAlertClick(alert)}
                >
                  <div className="flex items-start gap-3">
                    <div className="flex-shrink-0 mt-1">
                      {getAlertIcon(alert.level)}
                    </div>
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2 mb-1">
                        {getAlertBadge(alert.level)}
                        <span className="text-xs text-gray-500">
                          {new Date(alert.timestamp).toLocaleString()}
                        </span>
                      </div>
                      <p className="text-sm text-gray-900 break-words">
                        {alert.message}
                      </p>
                      {alert.actionUrl && (
                        <div className="flex items-center gap-1 mt-2 text-xs text-blue-600">
                          <ExternalLink className="w-3 h-3" />
                          <span>点击查看详情</span>
                        </div>
                      )}
                    </div>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="flex-shrink-0"
                      onClick={(e) => {
                        e.stopPropagation()
                        dismissAlert(alert.id)
                      }}
                    >
                      <X className="w-4 h-4" />
                    </Button>
                  </div>
                </div>
              ))}
            </div>
          </ScrollArea>
        )}
      </CardContent>
    </Card>
  )
}
